#[derive(Debug, Clone)]
pub struct SpineItem {
    pub idref: String,
    pub linear: bool,
}

impl SpineItem {
    pub fn new(idref: &str, linear: bool) -> Self {
        SpineItem {
            idref: idref.to_string(),
            linear,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Spine {
    pub items: Vec<SpineItem>,
    pub page_progression_direction: Option<String>,
}

impl Spine {
    pub fn new() -> Self {
        Spine {
            items: Vec::new(),
            page_progression_direction: None,
        }
    }

    pub fn add(&mut self, idref: &str, linear: bool) {
        self.items.push(SpineItem::new(idref, linear));
    }

    pub fn iter(&self) -> std::slice::Iter<'_, SpineItem> {
        self.items.iter()
    }
}
