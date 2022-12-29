use core::cell::RefCell;

use embassy_hal_common::Peripheral;
use embassy_stm32::{
    gpio::{self, Output},
    spi::{self, Spi},
    time::mhz,
};
use embassy_sync::{blocking_mutex::raw::RawMutex, channel::Channel, signal::Signal};

use crate::SPI_BUF_LEN;

pub type Result<T> = core::result::Result<T, spi::Error>;

pub enum Request {
    Enable,
    Disable,
    Transfer { txlen: usize, rxlen: usize },
}

pub async fn worker<'d, Periph, SckPin, MosiPin, MisoPin, CsPin, TxDma, RxDma, M>(
    mut peri: Periph,
    mut sck: SckPin,
    mut mosi: MosiPin,
    mut miso: MisoPin,
    mut cs: CsPin,
    mut txdma: TxDma,
    mut rxdma: RxDma,
    control: &'d Channel<M, Request, { core::mem::size_of::<Request>() }>,
    status: &'d Signal<M, Result<()>>,
    buf: &'d RefCell<[u8; SPI_BUF_LEN]>,
) where
    Periph: spi::Instance,
    SckPin: Peripheral<P = SckPin>,
    MosiPin: Peripheral<P = MosiPin>,
    MisoPin: Peripheral<P = MisoPin>,
    SckPin: spi::SckPin<Periph>,
    MosiPin: spi::MosiPin<Periph>,
    MisoPin: spi::MisoPin<Periph>,
    CsPin: gpio::Pin,
    TxDma: spi::TxDma<Periph>,
    RxDma: spi::RxDma<Periph>,
    M: RawMutex,
{
    let mut spi = None;

    loop {
        match control.recv().await {
            Request::Enable => {
                spi.get_or_insert_with(|| unsafe {
                    let spi = Spi::new(
                        peri.clone_unchecked(),
                        sck.clone_unchecked(),
                        mosi.clone_unchecked(),
                        miso.clone_unchecked(),
                        txdma.clone_unchecked(),
                        rxdma.clone_unchecked(),
                        mhz(1),
                        Default::default(),
                    );
                    let cs =
                        Output::new(cs.clone_unchecked(), gpio::Level::High, gpio::Speed::High);
                    debug!("spi enabled");
                    (spi, cs)
                });
            }
            Request::Disable => {
                // Dropping SPI instance automatically disables output driver on
                // all pins managed by SPI driver but does not disable CS pin.
                spi = None;
                unsafe {
                    cs.set_as_disconnected();
                }
                debug!("spi disabled");
            }
            Request::Transfer { txlen, rxlen } => {
                let (spi, cs) = spi.as_mut().unwrap();

                cs.set_low();

                if rxlen > txlen {
                    buf.borrow_mut()[txlen..txlen + rxlen].fill(0);
                }
                let n = txlen + rxlen;
                let r = { spi.transfer_in_place(&mut buf.borrow_mut()[..n]).await };

                /*let mut r = if txlen > 0 {
                    spi.write(&buf.borrow()[..txlen]).await
                } else {
                    Ok(())
                };
                if r.is_ok() {
                    r = if rxlen > 0 {
                        let buf = &mut buf.borrow_mut()[..rxlen];
                        spi.read(&mut buf[..]).await
                    } else {
                        Ok(())
                    };
                }*/

                cs.set_high();
                status.signal(r);
            }
        }
    }
}
