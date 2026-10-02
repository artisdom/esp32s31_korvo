# USB hub, keyboard and mouse

The default firmware now runs a Rust USB **host** on the native Type-A port.
USB-C (FT232R, `/dev/ttyUSB0`) remains the flashing/logging connection.

## Controls

Plug a USB hub into Type-A, then connect a keyboard and mouse to the hub.
Directly connected keyboards/mice are also supported. Select the **USB** tab
with touch, MODE, or F5. This page shows attached hub/keyboard/mouse counts,
key/modifier state, a bounded text field, mouse position/buttons/wheel and
report/overflow counters.

- F1–F6 select HOME, AUDIO, SD CARD, CAMERA, USB and ABOUT; Tab cycles pages.
- Left/Right select the previous/next audio track, SD file or camera video.
- Enter plays the selected file, or confirms an already displayed deletion.
- Escape cancels a pending deletion and stops playback/recording.
- Mouse movement controls the onscreen cursor; left-click operates tabs and
  media controls, including record, playback and the existing delete confirmation.
- Typed text uses the US layout, Shift/Caps Lock and Backspace. Held keys and
  mouse buttons do not repeat actions. Wheel movement is displayed on USB.

The UART `usb` command prints a complete input snapshot and host status.
Look for `usb: root connected`, device VID/PID messages and `usb: HID ready`.
Disconnecting a device releases its held keys/buttons. Reconnect a device
(or the whole hub) after an enumeration/class error to retry it.

## Supported topology and limits

The OTG-HS controller uses its native UTMI PHY in **full/low-speed-only mode**:
12 Mbit/s hubs/full-speed peripherals and 1.5 Mbit/s low-speed peripherals.
This avoids the Rust controller driver's currently unsupported high-speed
split transactions. USB 2.0 hubs, and the USB 2.0 fallback of compatible USB
3.x hubs, must negotiate full speed. SuperSpeed and 480 Mbit/s hub operation
are not implemented.

Two hub handlers, eight downstream ports per hub and four simultaneous HID
interfaces are supported. Composite keyboard/mouse receivers can use separate
HID interfaces. Up to 64 report fields, 64-byte input packets and 1024-byte
configuration/report descriptors are supported. Report protocol is preferred
for report IDs, NKRO keyboards, wheel data and signed wide mouse axes; boot
keyboard/mouse protocol is the fallback. Vendor-specific HID, game controllers,
absolute pointer devices, USB storage and keyboard LED output are outside this
implementation. Multiple keyboard report IDs that split modifier/key state
are not merged into one report. Text entry is a diagnostic field, not an editor.

The board's Type-A supply is limited to 500 mA by its TPS2051C switch; use a
self-powered hub when the attached equipment requires more power.

## Implementation and dependency setup

`src/usb_host.rs` owns bus enumeration, per-interface tasks and device topology
on core 1. `usb_hid.rs` decodes reports; `usb_input.rs` tracks input and queues
bounded press/click events. Core 0 consumes those events and owns all media/UI
operations. Cross-core state uses a critical-section mutex. The cursor is
removed before painting and restored after video/widgets to preserve pixels.

Device addresses remain reserved while their physical devices are attached,
even for unsupported interfaces or failed class setup. Class leases keep pipes
alive until their tasks stop. Removing a parent invalidates descendants, and
root removal allows addresses held by failed enumerations to be reclaimed.

The Embassy source checkout is pinned at upstream commit
`ae258ddd1b2a45715aef5ac12a70434b94e96139`. Patches add FS/LS-only controller
mode, correct UTMI frame timing, safe cancellation of interrupt/control IN
buffers, PRE control-stage spacing, selected HID interface construction and
preservation of simultaneous hub-port changes. Runtime time crates stay on the
published versions compatible with esp-rtos/embassy-executor; using the newer
checkout's timer-driver ABI stalled the first startup delay during bring-up.

Reproduce the local dependency changes with:

```sh
scripts/setup-usb-host.sh
cargo build --release                            # USB host + media
cargo build --release --features radio-wifi-ble   # also Wi-Fi/BLE
cargo build --release --no-default-features --features usb-device # legacy CDC
```

The host and legacy CDC role are mutually exclusive. The legacy CDC source is
retained in `src/usb_device.rs`; it is not the default Type-A behavior.

## Validation (2026-10-03)

- 25 host application tests passed, including seven HID/input tests for
  edge-triggered actions, multiple keyboards, unplug release, rollover,
  Shift/Caps/Backspace, report IDs, short reports, signed 16-bit mouse axes,
  wheel movement and NKRO bitmaps, plus the existing media regression tests.
- 83 Embassy USB-host unit tests passed.
- Release host + Wi-Fi/BLE and legacy CDC builds passed.
- Board startup, SD mount, Wi-Fi association/DHCP and BLE advertising passed
  with the host enabled. Camera AVI playback decoded all 55 frames of
  `VID00013.AVI` with zero skipped frames; LCD underruns remained zero.
- **Peripheral verification pending:** the captured UART run reported zero
  hubs/keyboards/mice and no root attach event. The user has a hub, keyboard
  and mouse available; enumeration, typing, clicking and hotplug still require
  those devices to be connected and tested. This is not yet a claim of verified
  USB peripheral operation on this board.
