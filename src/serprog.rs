use core::{cell::RefCell, mem};

use embassy_stm32::gpio::{self, Output};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, channel::Channel, signal::Signal};
use embassy_time::{Duration, Timer};

use crate::{spi, uart, PowerPin, SPI_BUF_LEN};

const S_ACK: u8 = 0x06;
const S_NAK: u8 = 0x15;
const S_CMD_NOP: u8 = 0x00; // No operation
const S_CMD_Q_IFACE: u8 = 0x01; // Query interface version
const S_CMD_Q_CMDMAP: u8 = 0x02; // Query supported commands bitma
const S_CMD_Q_PGMNAME: u8 = 0x03; // Query programmer name
const S_CMD_Q_SERBUF: u8 = 0x04; // Query Serial Buffer Size
const S_CMD_Q_BUSTYPE: u8 = 0x05; // Query supported bustypes
const S_CMD_Q_CHIPSIZE: u8 = 0x06; // Query supported chipsize (2^n forma
const S_CMD_Q_OPBUF: u8 = 0x07; // Query operation buffer size
const S_CMD_Q_WRNMAXLEN: u8 = 0x08; // Query Write to opbuf: Write-N maximum leng
const S_CMD_R_BYTE: u8 = 0x09; // Read a single byte
const S_CMD_R_NBYTES: u8 = 0x0A; // Read n bytes
const S_CMD_O_INIT: u8 = 0x0B; // Initialize operation buffer
const S_CMD_O_WRITEB: u8 = 0x0C; // Write opbuf: Write byte with addres
const S_CMD_O_WRITEN: u8 = 0x0D; // Write to opbuf: Write-N
const S_CMD_O_DELAY: u8 = 0x0E; // Write opbuf: udelay
const S_CMD_O_EXEC: u8 = 0x0F; // Execute operation buffer
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
    mut tx: uart::Writer<'static>,
    mut rx: uart::Reader<'static>,
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
        rx.read(&mut cmd[..]).await;

        debug!("process cmd {:#x}", cmd[0]);
        match cmd[0] {
            S_CMD_NOP => {
                tx.write(&[S_ACK]);
            }
            S_CMD_Q_IFACE => {
                tx.write(&[S_ACK, 1, 0]);
            }
            S_CMD_Q_PGMNAME => {
                tx.write(&PROGRAMMER_NAME_WITH_ACK);
            }
            S_CMD_Q_SERBUF => {
                // TODO: implement
                let mut resp = [S_ACK, 0, 0];
                let val: u16 = 32;
                resp[1..3].copy_from_slice(&val.to_le_bytes());
                tx.write(&resp);
            }
            S_CMD_Q_CMDMAP => {
                tx.write(&CMDMAP_WITH_ACK);
            }
            S_CMD_SYNCNOP => {
                tx.write(&[S_NAK, S_ACK]);
            }
            S_CMD_Q_BUSTYPE => {
                // Only SPI is supported
                tx.write(&[S_ACK, 8]);
            }
            S_CMD_Q_WRNMAXLEN | S_CMD_Q_RDNMAXLEN => {
                let x = ((SPI_BUF_LEN as u32) / 2).to_le_bytes();
                debug_assert_eq!(x[0], 0);
                tx.write(&[S_ACK, x[1], x[2], x[3]]);
            }
            S_CMD_S_BUSTYPE => {
                let mut bus = [0u8];
                rx.read(&mut bus[..]).await;
                tx.write(&[if bus[0] == 8 { S_ACK } else { S_NAK }]);
            }
            S_CMD_O_SPIOP => {
                let mut params = [0u8; 6];
                debug!("read params");
                rx.read(&mut params[..]).await;

                let slen = ((params[0] as u32) << 0)
                    | ((params[1] as u32) << 8)
                    | ((params[2] as u32) << 16);
                let rlen = ((params[3] as u32) << 0)
                    | ((params[4] as u32) << 8)
                    | ((params[5] as u32) << 16);

                debug!("spi tx {} rx {}", slen, rlen);

                assert!(slen as usize <= SPI_BUF_LEN);
                assert!(rlen as usize <= SPI_BUF_LEN);

                /*if slen > 0 {
                    let mut spi_buf = spi_buf.borrow_mut();
                    rx.read(&mut spi_buf[..slen as usize]).await;
                    if rlen > slen {
                        spi_buf[slen as usize..slen as usize + rlen as usize - 1].fill(0);
                    }
                }

                let n = max(slen, rlen);
                let direction = if slen == 0 {
                    TransferDirection::Rx
                } else if rlen == 0 {
                    TransferDirection::Tx
                } else {
                    TransferDirection::Both
                };*/

                if slen > 0 {
                    let mut spi_buf = spi_buf.borrow_mut();
                    rx.read(&mut spi_buf[..slen as usize]).await;
                }

                spi_control
                    .send(spi::Request::Transfer {
                        txlen: slen as usize,
                        rxlen: rlen as usize,
                    })
                    .await;

                match spi_status.wait().await {
                    Ok(()) => {
                        tx.write(&[S_ACK]);
                        if rlen > 0 {
                            let b = spi_buf.borrow();
                            tx.write(&b[slen as usize..slen as usize + rlen as usize]);
                        }
                    }
                    Err(e) => {
                        error!("spi transfer failed: {}", e);
                        tx.write(&[S_NAK]);
                    }
                }
            }
            S_CMD_S_PIN_STATE => {
                let mut enable = [0u8];
                rx.read(&mut enable[..]).await;
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

                tx.write(&[S_ACK]);
            }
            c => {
                warn!("unsupported cmd {:#x}", c);
                tx.write(&[S_NAK]);
                debug_assert!(false);
            }
        }

        debug!("cmd process done");
    }
}
