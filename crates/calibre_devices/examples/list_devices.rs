use calibre_devices::scanner::scan_usb_devices;

fn main() {
    println!("Scanning for USB devices...");
    let devices = scan_usb_devices();
    if devices.is_empty() {
        println!("No devices found.");
    } else {
        for device in devices {
            println!(
                "ID {:04x}:{:04x} {} {} SN:{}",
                device.vendor_id,
                device.product_id,
                device.manufacturer,
                device.model,
                device.serial
            );
        }
    }
}
