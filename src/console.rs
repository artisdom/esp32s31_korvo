//! Line commands on the existing UART0 debug console. TX stays with esp-println.
use crate::media::Command;
pub struct Console {
    line: heapless::String<256>,
}
impl Console {
    pub fn new() -> Self {
        Self {
            line: heapless::String::new(),
        }
    }
    pub fn poll(&mut self) -> Option<Command> {
        // This is the only RX consumer; neither esp-println nor the USB task
        // reads UART0. No clock/pin/baud settings are changed here.
        let uart = unsafe { &*esp_hal::peripherals::UART0::ptr() };
        while uart.status().read().rxfifo_cnt().bits() > 0 {
            let b = uart.fifo().read().rxfifo_rd_byte().bits();
            if b == b'\r' || b == b'\n' {
                if self.line.is_empty() {
                    continue;
                }
                let cmd = match self.line.as_str() {
                    "usb" => {
                        let s = crate::usb::INPUT.lock(|i| i.borrow().snapshot.clone());
                        esp_println::println!(
                            "usb status: hubs={} keyboards={} mice={} reports={} modifiers={:02x} last_key={:02x} mouse={:?} buttons={:02x} wheel={} dropped={} text={:?}",
                            s.hubs,
                            s.keyboards,
                            s.mice,
                            s.reports,
                            s.modifiers,
                            s.last_key,
                            s.mouse,
                            s.buttons,
                            s.wheel,
                            s.dropped,
                            s.text.as_str()
                        );
                        esp_println::println!("usb status: {}", crate::usb::USB_REPORT.read());
                        None
                    }
                    "play" => Some(Command::Play),
                    "stop" => Some(Command::Stop),
                    "next" => Some(Command::Next),
                    "prev" => Some(Command::Previous),
                    "record" => Some(Command::Record),
                    "replay" => Some(Command::Replay),
                    "rescan" => Some(Command::Refresh),
                    "demo" => Some(Command::Demo),
                    "tone" => Some(Command::Tone),
                    "mic" => Some(Command::Mic),
                    "video-play" => Some(Command::VideoPlay),
                    "video-replay" => Some(Command::VideoReplay),
                    "video-next" => Some(Command::VideoNext),
                    "video-prev" => Some(Command::VideoPrevious),
                    "file-play" => Some(Command::PlayFile),
                    "video" => Some(Command::VideoRecord),
                    "delete" => Some(Command::DeleteTrack),
                    "delete-file" => Some(Command::DeleteFile),
                    "confirm-delete" => Some(Command::ConfirmDelete),
                    "cancel-delete" => Some(Command::CancelDelete),
                    "file-next" => Some(Command::FileNext),
                    "file-prev" => Some(Command::FilePrevious),
                    text if text.starts_with("play ") => {
                        let name = text[5..].trim().to_ascii_uppercase();
                        Some(Command::PlayNamed(name))
                    }
                    _ => None,
                };
                esp_println::println!("console: {}", self.line);
                if cmd.is_none() && self.line.as_str() != "usb" {
                    esp_println::println!(
                        "commands: usb play stop next prev record replay video video-play video-replay video-next video-prev file-play rescan demo tone mic
                         delete delete-file confirm-delete cancel-delete file-next file-prev"
                    );
                }
                self.line.clear();
                return cmd;
            } else if b.is_ascii_graphic() || b == b' ' {
                if self.line.push(b as char).is_err() {
                    self.line.clear();
                }
            }
        }
        None
    }
}
