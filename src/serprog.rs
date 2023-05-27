use core::{cell::RefCell, mem};

use embassy_stm32::{
    gpio::{self, Output},
    usart::{rx_ringbuffered::RingBufferedUartRx, UartTx},
};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, channel::Channel, signal::Signal};
use embassy_time::{Duration, Timer};
use embedded_io::asynch::{Read, Write};

use crate::{spi, ControlUart, ControlUartRxDma, ControlUartTxDma, PowerPin, SPI_BUF_LEN};

const S_ACK: u8 = 0x06;
const S_NAK: u8 = 0x15;
const S_CMD_NOP: u8 = 0x00; // No operation
const S_CMD_Q_IFACE: u8 = 0x01; // Query interface version
const S_CMD_Q_CMDMAP: u8 = 0x02; // Query supported commands bitma
const S_CMD_Q_PGMNAME: u8 = 0x03; // Query programmer name
const S_CMD_Q_SERBUF: u8 = 0x04; // Query Serial Buffer Size
const S_CMD_Q_BUSTYPE: u8 = 0x05; // Query supported bustypes
const S_CMD_Q_WRNMAXLEN: u8 = 0x08; // Query Write to opbuf: Write-N maximum leng
const S_CMD_SYNCNOP: u8 = 0x10; // Special no-operation that returns NAK+A
const S_CMD_Q_RDNMAXLEN: u8 = 0x11; // Query read-n maximum length
const S_CMD_S_BUSTYPE: u8 = 0x12; // Set used bustype(s).
const S_CMD_O_SPIOP: u8 = 0x13; // Perform SPI operation.
const S_CMD_S_SPI_FREQ: u8 = 0x14; // Set SPI clock frequency
const S_CMD_S_PIN_STATE: u8 = 0x15; // Enable/disable output driver

const SUPPORTED_CMD: u32 = (1 << (S_CMD_NOP as u32))
    | (1 << (S_CMD_Q_IFACE as u32))
    | (1 << (S_CMD_Q_CMDMAP as u32))
    | (1 << (S_CMD_Q_PGMNAME as u32))
    | (1 << (S_CMD_Q_SERBUF as u32))
    | (1 << (S_CMD_Q_BUSTYPE as u32))
    | (1 << (S_CMD_Q_WRNMAXLEN as u32))
    | (1 << (S_CMD_SYNCNOP as u32))
    | (1 << (S_CMD_Q_RDNMAXLEN as u32))
    | (1 << (S_CMD_O_SPIOP as u32))
    | (1 << (S_CMD_S_BUSTYPE as u32))
    | (1 << (S_CMD_S_SPI_FREQ as u32))
    | (1 << (S_CMD_S_PIN_STATE as u32));

const PROGRAMMER_NAME_WITH_ACK: [u8; 17] = *b"\x06Rust serprog\x00\x00\x00\x00";

#[embassy_executor::task]
pub async fn run(
    mut tx: UartTx<'static, ControlUart, ControlUartTxDma>,
    mut rx: RingBufferedUartRx<'static, ControlUart, ControlUartRxDma>,
    power: PowerPin,
    spi_buf: &'static RefCell<[u8; SPI_BUF_LEN]>,
    spi_control: &'static Channel<NoopRawMutex, spi::Request, { mem::size_of::<spi::Request>() }>,
    spi_status: &'static Signal<NoopRawMutex, spi::Result<()>>,
) {
    static CMDMAP_WITH_ACK: [u8; 33] = {
        let mut x = [0u8; 33];
        let c = SUPPORTED_CMD.to_le_bytes();
        x[0] = S_ACK;
        x[1] = c[0];
        x[2] = c[1];
        x[3] = c[2];
        x[4] = c[3];
        x
    };

    let mut power = Output::new(power, gpio::Level::High, gpio::Speed::High);

    loop {
        let mut cmd = [0];
        rx.read_exact(&mut cmd[..]).await.unwrap();

        debug!("process cmd {:#x}", cmd[0]);
        match cmd[0] {
            S_CMD_NOP => {
                tx.write_all(&[S_ACK]).await.unwrap();
            }
            S_CMD_Q_IFACE => {
                tx.write_all(&[S_ACK, 1, 0]).await.unwrap();
            }
            S_CMD_Q_PGMNAME => {
                tx.write_all(&PROGRAMMER_NAME_WITH_ACK).await.unwrap();
            }
            S_CMD_Q_SERBUF => {
                // TODO: implement
                let mut resp = [S_ACK, 0, 0];
                let val: u16 = 32;
                resp[1..3].copy_from_slice(&val.to_le_bytes());
                tx.write_all(&resp).await.unwrap();
            }
            S_CMD_Q_CMDMAP => {
                tx.write_all(&CMDMAP_WITH_ACK).await.unwrap();
            }
            S_CMD_SYNCNOP => {
                tx.write_all(&[S_NAK, S_ACK]).await.unwrap();
            }
            S_CMD_Q_BUSTYPE => {
                // Only SPI is supported
                tx.write_all(&[S_ACK, 8]).await.unwrap();
            }
            S_CMD_Q_WRNMAXLEN | S_CMD_Q_RDNMAXLEN => {
                let x = ((SPI_BUF_LEN as u32) / 2).to_le_bytes();
                debug_assert_eq!(x[0], 0);
                tx.write_all(&[S_ACK, x[1], x[2], x[3]]).await.unwrap();
            }
            S_CMD_S_BUSTYPE => {
                let mut bus = [0u8];
                rx.read_exact(&mut bus[..]).await.unwrap();
                tx.write_all(&[if bus[0] == 8 { S_ACK } else { S_NAK }])
                    .await
                    .unwrap();
            }
            S_CMD_O_SPIOP => {
                let mut params = [0u8; 6];
                debug!("read params");
                rx.read_exact(&mut params[..]).await.unwrap();

                let slen = ((params[0] as u32) << 0)
                    | ((params[1] as u32) << 8)
                    | ((params[2] as u32) << 16);
                let rlen = ((params[3] as u32) << 0)
                    | ((params[4] as u32) << 8)
                    | ((params[5] as u32) << 16);

                debug!("spi tx {} rx {}", slen, rlen);

                assert!(slen as usize <= SPI_BUF_LEN);
                assert!(rlen as usize <= SPI_BUF_LEN);

                if slen > 0 {
                    let mut spi_buf = spi_buf.borrow_mut();
                    rx.read_exact(&mut spi_buf[..slen as usize]).await.unwrap();
                }

                spi_control
                    .send(spi::Request::Transfer {
                        txlen: slen as usize,
                        rxlen: rlen as usize,
                    })
                    .await;

                match spi_status.wait().await {
                    Ok(()) => {
                        tx.write_all(&[S_ACK]).await.unwrap();
                        if rlen > 0 {
                            let b = spi_buf.borrow();
                            tx.write_all(&b[slen as usize..slen as usize + rlen as usize])
                                .await
                                .unwrap();
                        }
                    }
                    Err(e) => {
                        error!("spi transfer failed: {}", e);
                        tx.write_all(&[S_NAK]).await.unwrap();
                    }
                }
            }
            S_CMD_S_PIN_STATE => {
                let mut enable = [0u8];
                rx.read_exact(&mut enable[..]).await.unwrap();
                let enable = enable[0] != 0;

                defmt::trace!("driver enable={}", enable);

                if enable {
                    power.set_low();
                    spi_control.send(spi::Request::Enable).await;
                    // Wait a bit to let the SPI flash power on.
                    Timer::after(Duration::from_millis(100)).await;
                } else {
                    spi_control.send(spi::Request::Disable).await;
                    power.set_high();
                }

                tx.write_all(&[S_ACK]).await.unwrap();
            }
            c => {
                warn!("unsupported cmd {:#x}", c);
                tx.write_all(&[S_NAK]).await.unwrap();
                debug_assert!(false);
            }
        }

        debug!("cmd process done");
    }
}
