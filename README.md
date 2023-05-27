# Flashrom serprog compatible programmer writter in Rust

### Features

- Support for SPI flash programming
- Works on Nucleo-L476RG board
- Capable of stable communication at 921600 UART baud rate and 10 MHz SPI
  frequency
- Supports disabling of output driver so you don't have to disconnect the
  programmer to boot the platform

### Building and running

Currently, only Nucleo-L476RG is supported, in the future, other boards as well
as non-ST platforms may be added.

If you don't have Rust already, install it from [rustup.rs](https://rustup.rs/).

Connect Nucleo and type `cargo run` to build and upload app to the device.

### Using the programmer

Programmer-host communication works through ST-Link serial port (USB-to-Serial
function available on all Nucleo boards).

Following pins are used (you can refer to
[Mbed OS documentation](https://os.mbed.com/platforms/ST-Nucleo-L476RG/#morpho-headers)
for pinout)

| PIN  | Function |
| ---- | -------- |
| PB13 | SCK      |
| PB15 | MOSI     |
| PB14 | MISO     |
| PB1  | CS#      |
| PB2  | POWER    |

> Note: POWER pin generally shouldn't be used for powering SPI flash directly
> but for controlling an external power source. Maximum power draw of Nucleo
> pins is limited to 20mA.

Example usage:

```shell
flashrom --progress -p serprog:dev=COM12:921600 -r bios_backup.rom
flashrom --progress -p serprog:dev=COM12:921600 -w coreboot.rom
```
