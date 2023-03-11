#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use core::{cell::RefCell, mem};

use embassy_executor::{Spawner, _export::StaticCell};
use embassy_stm32::{
    interrupt,
    peripherals::{DMA1_CH6, DMA1_CH7, PB2, USART2},
    rcc::{AHBPrescaler, APBPrescaler, ClockSrc, PLLClkDiv, PLLMul, PLLSource, PLLSrcDiv},
    usart::{self, DataBits, Parity, StopBits, Uart},
};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, channel::Channel, signal::Signal};

#[macro_use]
extern crate defmt;
extern crate defmt_rtt;
extern crate panic_probe;

mod serprog;
mod spi;
// mod uart;

const UART_BUF_LEN: usize = 16384;
const SPI_BUF_LEN: usize = 16384;
type PowerPin = PB2;
type ControlUart = USART2;
type ControlUartTxDma = DMA1_CH7;
type ControlUartRxDma = DMA1_CH6;

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    //static UART_STATE: StaticCell<uart::State> = StaticCell::new();
    static UART_TX_BUF: StaticCell<[u8; UART_BUF_LEN]> = StaticCell::new();
    static UART_RX_BUF: StaticCell<[u8; UART_BUF_LEN]> = StaticCell::new();
    static SPI_BUF: StaticCell<RefCell<[u8; SPI_BUF_LEN]>> = StaticCell::new();
    static SPI_CONTROL: StaticCell<
        Channel<NoopRawMutex, spi::Request, { mem::size_of::<spi::Request>() }>,
    > = StaticCell::new();
    static SPI_STATUS: StaticCell<Signal<NoopRawMutex, spi::Result<()>>> = StaticCell::new();

    info!("Hello from serprog");

    let p = embassy_stm32::init({
        let mut config = embassy_stm32::Config::default();
        config.rcc = embassy_stm32::rcc::Config {
            mux: ClockSrc::PLL(
                PLLSource::HSI16,
                PLLClkDiv::Div2,
                PLLSrcDiv::Div1,
                PLLMul::Mul10,
                None,
            ),
            ahb_pre: AHBPrescaler::NotDivided,
            apb1_pre: APBPrescaler::NotDivided,
            apb2_pre: APBPrescaler::NotDivided,
            pllsai1: Some((
                PLLMul::Mul8,
                PLLSrcDiv::Div1,
                Some(PLLClkDiv::Div2),
                Some(PLLClkDiv::Div2),
                Some(PLLClkDiv::Div8),
            )),
        };
        /*config.rcc = embassy_stm32::rcc::Config {
            mux: ClockSrc::PLL(
                PLLSource::HSI16,
                PLLClkDiv::Div2,
                PLLSrcDiv::Div1,
                PLLMul::Mul10,
                None,
            ),
            ahb_pre: AHBPrescaler::NotDivided,
            apb1_pre: APBPrescaler::NotDivided,
            apb2_pre: APBPrescaler::NotDivided,
            pllsai1: None,
        };*/
        config
    });

    let spi_control = SPI_CONTROL.init_with(Channel::new);
    let spi_status = SPI_STATUS.init_with(Signal::new);
    let spi_buf = SPI_BUF.init_with(|| RefCell::new([0u8; SPI_BUF_LEN]));
    let spi_fut = spi::worker(
        p.SPI2,
        p.PB13,
        p.PB15,
        p.PB14,
        p.PB1,
        p.DMA1_CH5,
        p.DMA1_CH4,
        &spi_control,
        &spi_status,
        spi_buf,
    );

    let uart = Uart::new(
        p.USART2,
        p.PA3,
        p.PA2,
        interrupt::take!(USART2),
        p.DMA1_CH7,
        p.DMA1_CH6,
        {
            let mut cfg = usart::Config::default();
            cfg.baudrate = 921600;
            cfg.data_bits = DataBits::DataBits8;
            cfg.parity = Parity::ParityNone;
            cfg.stop_bits = StopBits::STOP1;
            cfg
        },
    );
    let (uart_tx, uart_rx) = uart.split();
    let uart_rx = uart_rx.into_ring_buffered(UART_RX_BUF.init_with(|| [0u8; UART_BUF_LEN]));

    /*let (uart_fut, rx, tx) = uart::BufferedUart::new(
        uart,
        UART_STATE.init_with(Default::default),
        UART_TX_BUF.init_with(|| [0u8; UART_BUF_LEN]),
        UART_RX_BUF.init_with(|| [0u8; UART_BUF_LEN]),
    )
    .split();*/

    spawner.must_spawn(serprog::run(
        uart_tx,
        uart_rx,
        p.PB2,
        spi_buf,
        spi_control,
        spi_status,
    ));

    spi_fut.await;
}
