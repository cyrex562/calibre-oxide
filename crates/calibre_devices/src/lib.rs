use rusb::{Context, UsbContext};
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DeviceError {
    #[error("USB error: {0}")]
    Usb(#[from] rusb::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Other error: {0}")]
    Other(String),
}

#[derive(Debug, Clone)]
pub struct UsbDevice {
    pub bus_number: u8,
    pub address: u8,
    pub vendor_id: u16,
    pub product_id: u16,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial_number: Option<String>,
    pub class_code: u8,
    pub sub_class_code: u8,
    pub protocol_code: u8,
    pub is_mass_storage: bool,
}

pub struct DeviceScanner;

impl DeviceScanner {
    pub fn list_usb_devices() -> Result<Vec<UsbDevice>, DeviceError> {
        let context = Context::new()?;
        let devices = context.devices()?;
        let mut result = Vec::new();

        for device in devices.iter() {
            let device_desc = match device.device_descriptor() {
                Ok(d) => d,
                Err(_) => continue,
            };

            let handle = match device.open() {
                Ok(h) => h,
                Err(_) => {
                    result.push(UsbDevice {
                        bus_number: device.bus_number(),
                        address: device.address(),
                        vendor_id: device_desc.vendor_id(),
                        product_id: device_desc.product_id(),
                        manufacturer: None,
                        product: None,
                        serial_number: None,
                        class_code: device_desc.class_code(),
                        sub_class_code: device_desc.sub_class_code(),
                        protocol_code: device_desc.protocol_code(),
                        is_mass_storage: false, // Can't open, so assume false or unknown
                    });
                    continue;
                }
            };
            
            let timeout = Duration::from_secs(1);
            let languages = handle.read_languages(timeout).ok();
            let language = languages.as_ref().and_then(|l| l.first());

            let manufacturer = if let Some(lang) = language {
                handle.read_manufacturer_string(*lang, &device_desc, timeout).ok()
            } else {
                None
            };

            let product = if let Some(lang) = language {
                handle.read_product_string(*lang, &device_desc, timeout).ok()
            } else {
                None
            };

            let serial_number = if let Some(lang) = language {
                handle.read_serial_number_string(*lang, &device_desc, timeout).ok()
            } else {
                None
            };

            let mut is_mass_storage = false;
            // Check config descriptor for Mass Storage (Class 0x08)
            if let Ok(config) = device.active_config_descriptor() {
                for interface in config.interfaces() {
                    for descriptor in interface.descriptors() {
                        if descriptor.class_code() == 0x08 {
                            is_mass_storage = true;
                            break;
                        }
                    }
                    if is_mass_storage { break; }
                }
            } else if let Ok(config) = device.config_descriptor(0) {
                 // Fallback to first config
                 for interface in config.interfaces() {
                    for descriptor in interface.descriptors() {
                         if descriptor.class_code() == 0x08 {
                            is_mass_storage = true;
                            break;
                        }
                    }
                    if is_mass_storage { break; }
                }
            }

            result.push(UsbDevice {
                bus_number: device.bus_number(),
                address: device.address(),
                vendor_id: device_desc.vendor_id(),
                product_id: device_desc.product_id(),
                manufacturer,
                product,
                serial_number,
                class_code: device_desc.class_code(),
                sub_class_code: device_desc.sub_class_code(),
                protocol_code: device_desc.protocol_code(),
                is_mass_storage,
            });
        }

        Ok(result)
    }
}
