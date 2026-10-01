# USB 2.0 HS Device (Type-A Port)

## Hardware

The ESP32-S31 has a native USB 2.0 High-Speed PHY on dedicated pins
(no GPIO muxing). The Type-A port is wired directly to USB_DP/USB_DM.
A TPS2051C power switch provides VBUS (500 mA) to the host.

## Software stack

```
esp_hal::usb::otg::Usb           — Synopsys OTG-HS controller
  └── embassy_usb_device::Driver  — embassy-usb Driver trait impl
      └── embassy_usb::Builder    — USB device stack
          └── CdcAcmClass         — CDC-ACM serial class
```

## Implementation

Runs as a dedicated embassy task on **core 1** (set up via
`esp_rtos::start_second_core`). This keeps USB interrupt handling off
core 0 where the UI/audio loop runs.

```rust
#[embassy_executor::task]
pub async fn usb_task(usb_hs: esp_hal::peripherals::USB_HS<'static>) {
    let usb = Usb::new_hs(usb_hs);
    let driver = Driver::new(usb, &mut ep_out_buffer, Config::default());

    let mut config = embassy_usb::Config::new(0x16c0, 0x27dd);
    config.manufacturer = Some("esp-rs");
    config.product = Some("ESP32-S31-Korvo demo");
    config.max_packet_size_0 = 64;

    let mut builder = Builder::new(driver, config, ...);
    let mut class = CdcAcmClass::new(&mut builder, &mut state, 512);
    let mut usb_dev = builder.build();

    join(usb_dev.run(), echo_loop(&mut class)).await;
}
```

## Usage

Connect a USB cable to the Type-A port:
```
screen /dev/ttyACM0    # any baud rate (CDC ignores it)
```

- Typed text echoes back in **UPPER CASE**
- Send `?` for a status report (uptime, rx/tx byte counts, loop counter)
- Live counters shown on the LCD UI

## Key details

- `USB_HS` peripheral is consumed directly (no pin muxing needed)
- Endpoint buffer must be `'static` — use `static mut` with unsafe
  or a `StaticCell`
- HS requires `max_packet_size = 512` for CDC bulk endpoints
- The `embassy-usb-synopsys-otg` crate needs the `host` feature enabled
  (already handled by esp-hal's `__usb_otg` feature)
