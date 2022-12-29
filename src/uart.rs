use core::{
    cmp::min,
    future::{poll_fn, Future},
    task::Poll,
};

use embassy_hal_common::atomic_ring_buffer::{
    Reader as RingReader, RingBuffer, Writer as RingWriter,
};
use embassy_stm32::usart::{self, BasicInstance, Uart, UartRx, UartTx};
use embassy_sync::waitqueue::AtomicWaker;

pub struct State {
    tx: RingBuffer,
    rx: RingBuffer,
    rx_waker: AtomicWaker,
    tx_waker: AtomicWaker,
}

impl Default for State {
    fn default() -> Self {
        Self {
            tx: RingBuffer::new(),
            rx: RingBuffer::new(),
            rx_waker: AtomicWaker::new(),
            tx_waker: AtomicWaker::new(),
        }
    }
}

pub struct BufferedUart<'d, T, TxDma, RxDma>
where
    T: BasicInstance,
{
    uart: Uart<'d, T, TxDma, RxDma>,
    state: &'d State,
}

impl<'d, T, TxDma, RxDma> BufferedUart<'d, T, TxDma, RxDma>
where
    T: BasicInstance,
    TxDma: usart::TxDma<T>,
    RxDma: usart::RxDma<T>,
{
    pub fn new(
        uart: Uart<'d, T, TxDma, RxDma>,
        state: &'d mut State,
        tx_buf: &'d mut [u8],
        rx_buf: &'d mut [u8],
    ) -> Self {
        assert_eq!(tx_buf.len(), rx_buf.len());
        assert!(tx_buf.len() > 0 && tx_buf.len() % 2 == 0);

        unsafe {
            state.tx.init(tx_buf.as_mut_ptr(), tx_buf.len());
            state.rx.init(rx_buf.as_mut_ptr(), rx_buf.len());
        }

        Self { uart, state }
    }

    pub fn split(self) -> (impl Future + 'd, Reader<'d>, Writer<'d>) {
        let Self { uart, state } = self;

        let (rxbuf_writer, rxbuf_reader) = unsafe { (state.rx.writer(), state.rx.reader()) };
        let (txbuf_writer, txbuf_reader) = unsafe { (state.tx.writer(), state.tx.reader()) };

        (
            worker(uart, rxbuf_writer, txbuf_reader, state),
            Reader {
                state,
                rxbuf_reader,
            },
            Writer {
                state,
                txbuf_writer,
            },
        )
    }
}

async fn worker<'d, T, TxDma, RxDma>(
    uart: Uart<'d, T, TxDma, RxDma>,
    rxbuf_writer: RingWriter<'d>,
    txbuf_reader: RingReader<'d>,
    state: &'d State,
) where
    T: BasicInstance,
    TxDma: usart::TxDma<T>,
    RxDma: usart::RxDma<T>,
{
    let (tx, rx) = uart.split();

    embassy_futures::join::join(
        worker_tx(tx, txbuf_reader, state),
        worker_rx(rx, rxbuf_writer, state),
    )
    .await;
}

async fn worker_rx<'d, T, RxDma>(
    mut rx: UartRx<'d, T, RxDma>,
    mut rxbuf_writer: RingWriter<'d>,
    state: &'d State,
) where
    T: BasicInstance,
    RxDma: usart::RxDma<T>,
{
    loop {
        let dest = rxbuf_writer.push_slice();
        defmt::assert!(!dest.is_empty(), "uart rx buf overrun");

        trace!("uart read start max {}", dest.len());
        let n = rx.read_until_idle(dest).await.unwrap();
        trace!("uart read {}", n);
        if n == 0 {
            continue;
        }

        rxbuf_writer.push_done(n);
        state.rx_waker.wake();
    }
}

async fn worker_tx<'d, T, TxDma>(
    mut tx: UartTx<'d, T, TxDma>,
    mut txbuf_reader: RingReader<'d>,
    state: &'d State,
) where
    T: BasicInstance,
    TxDma: usart::TxDma<T>,
{
    loop {
        poll_fn(|cx| {
            if !txbuf_reader.pop_slice().is_empty() {
                Poll::Ready(())
            } else {
                state.tx_waker.register(cx.waker());
                Poll::Pending
            }
        })
        .await;

        let data = txbuf_reader.pop_slice();
        tx.write(data).await.unwrap();
        let len = data.len();
        txbuf_reader.pop_done(len);
    }
}

pub struct Reader<'d> {
    state: &'d State,
    rxbuf_reader: RingReader<'d>,
}

impl Reader<'_> {
    #[allow(dead_code)]
    pub async fn wait(&mut self) -> &[u8] {
        poll_fn(|cx| {
            let slice = self.rxbuf_reader.pop_slice();
            if slice.is_empty() {
                self.state.rx_waker.register(cx.waker());
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
        .await;

        self.rxbuf_reader.pop_slice()
    }

    pub async fn read(&mut self, buf: &mut [u8]) {
        if buf.is_empty() {
            return;
        }

        let len = buf.len();

        poll_fn(|cx| {
            let slice = self.rxbuf_reader.pop_slice();
            if let Some(src) = slice.get(..len) {
                buf.copy_from_slice(src);
                self.discard(len);
                Poll::Ready(())
            } else {
                self.state.rx_waker.register(cx.waker());
                Poll::Pending
            }
        })
        .await
    }

    pub fn discard(&mut self, n: usize) {
        self.rxbuf_reader.pop_done(n)
    }
}

pub struct Writer<'d> {
    state: &'d State,
    txbuf_writer: RingWriter<'d>,
}

impl Writer<'_> {
    pub fn write(&mut self, data: &[u8]) {
        push_to_ringbuf(&mut self.txbuf_writer, data);
        self.state.tx_waker.wake();
    }
}

fn push_to_ringbuf(writer: &mut RingWriter, data: &[u8]) {
    let mut do_write = |data: &[u8]| {
        let dest = writer.push_slice();
        let n = min(data.len(), dest.len());
        if n == 0 {
            return 0;
        }
        dest[..n].copy_from_slice(&data[..n]);
        writer.push_done(n);

        n
    };

    let mut written = 0;
    loop {
        let n = do_write(&data[written..]);
        written += n;
        if n == 0 || written == data.len() {
            break;
        }
    }

    if written != data.len() {
        defmt::panic!("tx buffer overrun, wrote {} out of {}", written, data.len());
    }
}
