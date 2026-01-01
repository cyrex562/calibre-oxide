use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputProfile {
    pub name: String,
    pub short_name: String,
    pub description: String,
    pub screen_size: (u32, u32),
    pub dpi: f64,
    pub fbase: f64,
    pub fsizes: Vec<f64>,
}

impl Default for InputProfile {
    fn default() -> Self {
        InputProfile {
            name: "Default Input Profile".to_string(),
            short_name: "default".to_string(),
            description: "Default input profile".to_string(),
            screen_size: (1600, 1200),
            dpi: 100.0,
            fbase: 12.0,
            fsizes: vec![5.0, 7.0, 9.0, 12.0, 13.5, 17.0, 20.0, 22.0, 24.0],
        }
    }
}

impl InputProfile {
    pub fn new_sony_reader() -> Self {
        InputProfile {
            name: "Sony Reader".to_string(),
            short_name: "sony".to_string(),
            description: "Sony PRS line".to_string(),
            screen_size: (584, 754),
            dpi: 168.451,
            fbase: 12.0,
            fsizes: vec![7.5, 9.0, 10.0, 12.0, 15.5, 20.0, 22.0, 24.0],
        }
    }

    pub fn new_kindle() -> Self {
        InputProfile {
            name: "Kindle".to_string(),
            short_name: "kindle".to_string(),
            description: "Amazon Kindle".to_string(),
            screen_size: (525, 640),
            dpi: 168.451,
            fbase: 16.0,
            fsizes: vec![12.0, 12.0, 14.0, 16.0, 18.0, 20.0, 22.0, 24.0],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputProfile {
    pub name: String,
    pub short_name: String,
    pub description: String,
    pub screen_size: (u32, u32),
    pub comic_screen_size: (u32, u32),
    pub dpi: f64,
    pub touchscreen: bool,
    pub ratings_char: char,
    pub empty_ratings_char: char,
    pub mobi_ems_per_blockquote: f64,
}

impl Default for OutputProfile {
    fn default() -> Self {
        OutputProfile {
            name: "Default Output Profile".to_string(),
            short_name: "default".to_string(),
            description: "Default output profile".to_string(),
            screen_size: (1600, 1200),
            comic_screen_size: (584, 754),
            dpi: 100.0,
            touchscreen: false,
            ratings_char: '*',
            empty_ratings_char: ' ',
            mobi_ems_per_blockquote: 1.0,
        }
    }
}

impl OutputProfile {
    pub fn new_ipad() -> Self {
        OutputProfile {
            name: "iPad".to_string(),
            short_name: "ipad".to_string(),
            description: "iPad".to_string(),
            screen_size: (768, 1024),
            comic_screen_size: (768, 1024),
            dpi: 132.0,
            touchscreen: true,
            ratings_char: '\u{2605}', // filled star
            empty_ratings_char: '\u{2606}', // hollow star
            ..Default::default()
        }
    }

    pub fn new_kindle() -> Self {
        OutputProfile {
            name: "Kindle".to_string(),
            short_name: "kindle".to_string(),
            description: "Amazon Kindle".to_string(),
            screen_size: (525, 640),
            comic_screen_size: (525, 640), // Typically same or adjusted
            dpi: 168.451,
            mobi_ems_per_blockquote: 2.0,
            ratings_char: '\u{2605}',
            empty_ratings_char: '\u{2606}',
            ..Default::default()
        }
    }
}
