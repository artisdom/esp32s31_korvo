# Native Type-A USB

The ESP32-S31 USB OTG-HS controller and native PHY connect to the board's
Type-A port. A TPS2051C supplies downstream VBUS with a 500 mA current limit.
The separate USB-C FT232R connection provides flashing and UART logs.

The default firmware is now a **USB host** for hubs, keyboards and mice.
See [USB host input](17-usb-host-input.md) for controls, implementation,
dependency setup, validation and limitations.

The previous CDC-ACM device implementation is retained as an optional build:

```sh
cargo build --release --no-default-features --features usb-device
```

It runs on core 1, echoes received ASCII in upper case and responds to `?`
with a system report. Counters remain available to the LCD. Shared text buffers
now use a mutex rather than unsynchronized cross-core access. Host and device
features cannot be enabled together. The board's documented Type-A use is as a
host; this optional device role preserves the earlier experimental software.
