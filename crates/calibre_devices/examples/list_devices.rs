use calibre_devices::DeviceScanner;

fn main() {
    println!("Scanning for USB devices...");
    match DeviceScanner::list_usb_devices() {
        Ok(devices) => {
            if devices.is_empty() {
                println!("No devices found.");
            } else {
                for device in devices {
                    println!("--------------------------------");
                    println!("Bus {:03} Address {:03}", device.bus_number, device.address);
                    println!(
                        "ID {:04x}:{:04x} Class:{:02x} Sub:{:02x} Proto:{:02x} Storage:{} {} {}",
                        device.vendor_id,
                        device.product_id,
                        device.class_code,
                        device.sub_class_code,
                        device.protocol_code,
                        device.is_mass_storage,
                        device.manufacturer.as_deref().unwrap_or("[Unknown Vendor]"),
                        device.product.as_deref().unwrap_or("[Unknown Product]")
                    );
                    if let Some(sn) = &device.serial_number {
                        println!("Serial: {}", sn);
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("Error scanning devices: {}", e);
            eprintln!("(Note: You might need 'libusb-1.0-0-dev' installed and udev rules/permissions to access devices)");
        }
    }
}
