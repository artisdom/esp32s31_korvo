//! Pure Rust FS/LS USB host: hubs and boot/report-protocol HID interfaces.
extern crate alloc;
use super::{CONNECTED, INPUT, RX_BYTES, USB_REPORT};
use alloc::boxed::Box;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use embassy_executor::Spawner;
use embassy_futures::select::select;
use embassy_time::{Duration, Timer, with_timeout};
use embassy_usb_driver::host::{DeviceEvent, UsbHostController};
use embassy_usb_host::{
    BusHandle, BusRoute, BusState, bus,
    class::{
        hid::{HidHost, HidInfo, KeyboardReport, MouseReport, PROTOCOL_BOOT, ReportDescriptor},
        hub::{HubEvent, HubHandler},
    },
    descriptor::ConfigurationDescriptorChain,
    handler::{EnumerationInfo, HandlerEvent},
};
use esp_hal::usb::otg::{Usb, embassy_usb_host::Driver};
use esp_println::println;
type Allocator = <Driver<'static> as UsbHostController<'static>>::Allocator;
type Handle = BusHandle<'static, Allocator>;
type Hid = HidHost<'static, Handle>;
type Hub = HubHandler<'static, Allocator, 8>;
static BUS: BusState = BusState::new();
static GENERATION: AtomicU32 = AtomicU32::new(0);
static ALIVE: [AtomicBool; 128] = [const { AtomicBool::new(false) }; 128];
static PARENT: [AtomicU8; 128] = [const { AtomicU8::new(0) }; 128];
static REFS: [AtomicU8; 128] = [const { AtomicU8::new(0) }; 128];
static RESERVED: [AtomicBool; 128] = [const { AtomicBool::new(false) }; 128];
static SLOTS: AtomicU8 = AtomicU8::new(0);
fn alive(addr: u8, generation: u32) -> bool {
    if GENERATION.load(Ordering::Acquire) != generation {
        return false;
    }
    let mut a = addr;
    for _ in 0..8 {
        if !ALIVE[a as usize].load(Ordering::Acquire) {
            return false;
        }
        a = PARENT[a as usize].load(Ordering::Acquire);
        if a == 0 {
            return true;
        }
    }
    false
}
async fn removed(addr: u8, generation: u32) {
    while alive(addr, generation) {
        Timer::after_millis(10).await;
    }
}
struct Lease {
    bus: Handle,
    addr: u8,
}
impl Lease {
    fn new(bus: &Handle, addr: u8) -> Self {
        REFS[addr as usize].fetch_add(1, Ordering::AcqRel);
        Self {
            bus: bus.clone(),
            addr,
        }
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if REFS[self.addr as usize].fetch_sub(1, Ordering::AcqRel) == 1 {
            ALIVE[self.addr as usize].store(false, Ordering::Release);
            self.bus.free_address(self.addr);
        }
    }
}
struct HidDevice {
    driver: Hid,
    lease: Lease,
    slot: usize,
    protocol: u8,
    kind: u8,
}
impl Drop for HidDevice {
    fn drop(&mut self) {
        INPUT.lock(|i| {
            let mut i = i.borrow_mut();
            i.release(self.slot);
            if self.kind & 1 != 0 {
                i.snapshot.keyboards = i.snapshot.keyboards.saturating_sub(1);
            }
            if self.kind & 2 != 0 {
                i.snapshot.mice = i.snapshot.mice.saturating_sub(1);
                if i.snapshot.mice == 0 {
                    i.snapshot.mouse = None;
                }
            }
        });
        SLOTS.fetch_and(!(1 << self.slot), Ordering::AcqRel);
    }
}
struct HubDevice {
    driver: Hub,
    lease: Lease,
}
impl Drop for HubDevice {
    fn drop(&mut self) {
        INPUT.lock(|i| {
            let mut i = i.borrow_mut();
            i.snapshot.hubs = i.snapshot.hubs.saturating_sub(1);
        });
    }
}
fn slot() -> Option<usize> {
    for n in 0..4 {
        let bit = 1 << n;
        if SLOTS.fetch_or(bit, Ordering::AcqRel) & bit == 0 {
            return Some(n);
        }
    }
    None
}
async fn register(
    spawner: Spawner,
    handle: &Handle,
    info: EnumerationInfo,
    cfg: &[u8],
    parent: u8,
) {
    let addr = info.device_address;
    let generation = GENERATION.load(Ordering::Acquire);
    PARENT[addr as usize].store(parent, Ordering::Release);
    ALIVE[addr as usize].store(true, Ordering::Release);
    // Keep the address pinned while tasks are constructed, even on spawn failure.
    let registration = Lease::new(handle, addr);
    println!(
        "usb: addr={} parent={} VID={:04x} PID={:04x} speed={:?}",
        addr,
        parent,
        info.device_desc.vendor_id,
        info.device_desc.product_id,
        info.speed()
    );
    if let Ok(chain) = ConfigurationDescriptorChain::try_from_slice(cfg) {
        if info.device_desc.device_class == 9
            || chain.iter_interface().any(|i| i.interface_class == 9)
        {
            match with_timeout(Duration::from_secs(3), Hub::try_register(handle, &info)).await {
                Ok(Ok(driver)) => {
                    INPUT.lock(|i| i.borrow_mut().snapshot.hubs += 1);
                    let dev = HubDevice {
                        driver,
                        lease: Lease::new(handle, addr),
                    };
                    if let Ok(token) = hub_task(dev, spawner, generation) {
                        spawner.spawn(token);
                    } else {
                        println!("usb: hub task capacity reached");
                    }
                }
                _ => println!("usb: hub registration failed"),
            }
        } else {
            for iface in chain.iter_interface().filter(|i| i.interface_class == 3) {
                let Some(ep) = iface
                    .iter_endpoints()
                    .find(|e| e.is_in() && e.transfer_type() == 3)
                else {
                    continue;
                };
                if ep.max_packet_size > 64 {
                    println!("usb: HID packet exceeds 64 bytes");
                    continue;
                }
                let Some(n) = slot() else {
                    println!("usb: HID interface capacity reached");
                    break;
                };
                let len = iface
                    .iter_descriptors()
                    .find_map(|(_, b)| {
                        if b.len() >= 9 && b[1] == 0x21 {
                            Some(u16::from_le_bytes([b[7], b[8]]))
                        } else {
                            None
                        }
                    })
                    .unwrap_or(0);
                let hi = HidInfo {
                    interface_number: iface.interface_number,
                    interrupt_in_ep: ep.endpoint_address,
                    interrupt_in_mps: ep.max_packet_size,
                    interrupt_in_interval: ep.interval,
                    report_descriptor_len: len,
                };
                match Hid::new_with_info(handle, &hi, &info) {
                    Ok(driver) => {
                        let protocol = if iface.interface_subclass == 1 {
                            iface.interface_protocol
                        } else {
                            0
                        };
                        let dev = HidDevice {
                            driver,
                            lease: Lease::new(handle, addr),
                            slot: n,
                            protocol,
                            kind: 0,
                        };
                        if let Ok(token) = hid_task(dev, generation, len) {
                            spawner.spawn(token);
                        } else {
                            println!("usb: HID task capacity reached");
                        }
                    }
                    Err(e) => {
                        SLOTS.fetch_and(!(1 << n), Ordering::AcqRel);
                        println!("usb: HID allocation {:?}", e);
                    }
                }
            }
        }
    }
    // Even unsupported interfaces retain their USB address until physical removal.
    // Releasing it earlier would put two connected devices at the same address.
    RESERVED[addr as usize].store(true, Ordering::Release);
    core::mem::forget(registration);
    USB_REPORT.write("USB host ready; open USB tab for input");
}
#[embassy_executor::task(pool_size = 2)]
async fn hub_task(mut dev: HubDevice, spawner: Spawner, generation: u32) {
    let addr = dev.lease.addr;
    let handle = dev.lease.bus.clone();
    let run = async {
        let mut cfg = [0; 1024];
        loop {
            match dev.driver.wait_for_event().await {
                Ok(HandlerEvent::HandlerEvent(HubEvent::DeviceDetected { port, speed })) => {
                    if port >= 8 {
                        println!("usb: hub port {} exceeds supported 8", port);
                        continue;
                    }
                    Timer::after_millis(100).await; // connection debounce before resetting downstream port
                    match with_timeout(
                        Duration::from_secs(3),
                        dev.driver.enumerate_port(&mut cfg, port, speed),
                    )
                    .await
                    {
                        Ok(Ok((info, n))) => {
                            register(spawner, &handle, info, &cfg[..n], addr).await
                        }
                        e => println!("usb: hub {} port {} enumeration {:?}", addr, port, e),
                    }
                }
                Ok(HandlerEvent::HandlerEvent(HubEvent::DeviceRemoved { address, .. })) => {
                    if let Some(a) = address {
                        ALIVE[a.get() as usize].store(false, Ordering::Release);
                    }
                }
                Ok(HandlerEvent::HandlerDisconnected) => break,
                Ok(_) => {}
                Err(e) => {
                    println!("usb: hub {} error {:?}", addr, e);
                    break;
                }
            }
        }
    };
    select(run, removed(addr, generation)).await;
    // A class error does not electrically disconnect the hub or its children.
    // Keep their addresses reserved until a real detach; replug to retry the class.
    println!("usb: hub {} handler stopped", addr);
}
#[embassy_executor::task(pool_size = 4)]
async fn hid_task(mut dev: HidDevice, generation: u32, desc_len: u16) {
    let addr = dev.lease.addr;
    let slot = dev.slot;
    let run = async {
        let mut desc_buf = [0; 1024];
        let mut descriptor = None;
        // Prefer report protocol for wheel movement, report IDs and wide mouse axes.
        if desc_len > 0 && desc_len <= 1024 {
            if let Ok(Ok(bytes)) = with_timeout(
                Duration::from_secs(2),
                dev.driver.fetch_report_descriptor(&mut desc_buf),
            )
            .await
            {
                let parsed = Box::new(ReportDescriptor::<64>::parse(bytes));
                let keyboard = parsed
                    .fields()
                    .any(|f| f.usage_page == 7 && f.flags & 1 == 0);
                let mouse = parsed
                    .fields()
                    .any(|f| f.usage_page == 1 && f.usage_min == 0x30 && f.flags & 4 != 0);
                if keyboard || mouse {
                    dev.kind = (keyboard as u8) | ((mouse as u8) << 1);
                    descriptor = Some(parsed);
                }
            }
        }
        let boot = descriptor.is_none();
        if boot {
            if !(dev.protocol == 1 || dev.protocol == 2)
                || !matches!(
                    with_timeout(
                        Duration::from_secs(2),
                        dev.driver.set_protocol(PROTOCOL_BOOT)
                    )
                    .await,
                    Ok(Ok(()))
                )
            {
                println!("usb: HID {} unsupported descriptor / boot protocol", addr);
                return;
            }
            dev.kind = dev.protocol;
        }
        INPUT.lock(|i| {
            let mut i = i.borrow_mut();
            if dev.kind & 1 != 0 {
                i.snapshot.keyboards += 1;
            }
            if dev.kind & 2 != 0 {
                i.snapshot.mice += 1;
            }
        });
        let _ = with_timeout(Duration::from_secs(1), dev.driver.set_idle(0, 0)).await;
        println!(
            "usb: HID ready addr={} slot={} kind={} boot={} protocol={}",
            addr, slot, dev.kind, boot, dev.protocol
        );
        let mut buf = [0; 64];
        loop {
            match dev.driver.read(&mut buf).await {
                Ok(n) => {
                    RX_BYTES.fetch_add(n as u32, Ordering::Relaxed);
                    INPUT.lock(|i| {
                        let mut i = i.borrow_mut();
                        i.snapshot.reports += 1;
                        if boot && dev.protocol == 1 {
                            if let Some(r) = KeyboardReport::parse(&buf[..n]) {
                                let mut keys = [0; 32];
                                for k in r.keycodes {
                                    keys[k as usize / 8] |= 1 << (k % 8);
                                }
                                i.keyboard(slot, r.modifiers, keys);
                            }
                        } else if boot && dev.protocol == 2 {
                            if let Some(r) = MouseReport::parse(&buf[..n]) {
                                i.mouse(slot, r.buttons, r.x as i32, r.y as i32, r.wheel as i32);
                            }
                        } else if let Some(d) = descriptor.as_ref() {
                            crate::usb_hid::decode(d, &buf[..n], &mut i, slot);
                        }
                    });
                }
                Err(e) => {
                    println!("usb: HID {} stopped {:?}", addr, e);
                    break;
                }
            }
        }
    };
    select(run, removed(addr, generation)).await;
    println!("usb: HID {} slot {} detached", addr, slot);
}
#[embassy_executor::task]
pub async fn usb_task(usb_hs: esp_hal::peripherals::USB_HS<'static>) {
    let driver = Driver::new_full_speed(Usb::new_hs(usb_hs));
    let (mut controller, handle) = bus(driver, &BUS);
    // SAFETY: this Embassy task is polled exclusively by the core-1 executor.
    let spawner = unsafe { Spawner::for_current_executor().await };
    println!("usb: FS/LS host ready on Type-A; hubs/keyboard/mouse");
    USB_REPORT.write("USB host: plug hub, keyboard or mouse into Type-A");
    spawner.spawn(address_cleanup(handle.clone()).expect("USB address cleanup task"));
    let mut cfg = [0; 1024];
    loop {
        match controller.wait_for_device_event().await {
            DeviceEvent::Connected(speed) => {
                CONNECTED.store(true, Ordering::Release);
                println!("usb: root connected {:?}", speed);
                match with_timeout(
                    Duration::from_secs(4),
                    handle.enumerate(BusRoute::Direct(speed), &mut cfg),
                )
                .await
                {
                    Ok(Ok((info, n))) => register(spawner, &handle, info, &cfg[..n], 0).await,
                    e => {
                        println!("usb: root enumeration {:?}", e);
                        USB_REPORT.write("USB enumeration failed; unplug and reconnect");
                    }
                }
            }
            DeviceEvent::Disconnected => {
                CONNECTED.store(false, Ordering::Release);
                GENERATION.fetch_add(1, Ordering::AcqRel);
                for a in &ALIVE {
                    a.store(false, Ordering::Release);
                }
                // All child tasks see the generation change and relinquish pipes/addresses.
                Timer::after_millis(30).await;
                // Failed enumerations may already have sent SET_ADDRESS. They stay
                // reserved until root detach to avoid colliding with a live device.
                for addr in 1..128u8 {
                    if REFS[addr as usize].load(Ordering::Acquire) == 0 {
                        handle.free_address(addr);
                    }
                }
                USB_REPORT.write("USB host: plug hub, keyboard or mouse into Type-A");
                println!("usb: root disconnected");
            }
            DeviceEvent::Overcurrent => {
                USB_REPORT.write("USB overcurrent; disconnect load and reconnect hub");
                println!("usb: root overcurrent");
            }
            e => println!("usb: root event {:?}", e),
        }
    }
}

/// Address ownership follows physical topology, independently of class-task success.
#[embassy_executor::task]
async fn address_cleanup(handle: Handle) {
    loop {
        let generation = GENERATION.load(Ordering::Acquire);
        for addr in 1..128u8 {
            if RESERVED[addr as usize].load(Ordering::Acquire)
                && !alive(addr, generation)
                && RESERVED[addr as usize].swap(false, Ordering::AcqRel)
            {
                // Release the registration reference; class leases drop their pipes first.
                drop(Lease {
                    bus: handle.clone(),
                    addr,
                });
            }
        }
        Timer::after_millis(10).await;
    }
}
