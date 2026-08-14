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

    /// Insert `idref` at `index` (clamped to the current length). Used by
    /// `mobi::writer8::toc::TocAdder` for `mobi_toc_at_start`
    /// (`oeb.spine.insert(0, tocitem, linear=True)` in Python).
    pub fn insert(&mut self, index: usize, idref: &str, linear: bool) {
        let idx = index.min(self.items.len());
        self.items.insert(idx, SpineItem::new(idref, linear));
    }

    /// Remove every spine entry referencing `idref` (Python's
    /// `oeb.spine.remove(item)`, keyed here by manifest id since this
    /// port's `SpineItem` stores `idref`, not an item reference).
    pub fn remove_by_idref(&mut self, idref: &str) {
        self.items.retain(|i| i.idref != idref);
    }

    /// Port of `oeb.spine.index(item)`: the position of `idref`'s entry,
    /// if any.
    pub fn index_of(&self, idref: &str) -> Option<usize> {
        self.items.iter().position(|i| i.idref == idref)
    }

    pub fn iter(&self) -> std::slice::Iter<'_, SpineItem> {
        self.items.iter()
    }
}
