use crate::tables::TableType;

pub trait Field {
    fn name(&self) -> &str;
    fn table_type(&self) -> TableType;

    // Common accessors
    fn is_multiple(&self) -> bool {
        false
    }
}

pub struct BasicField {
    pub name: String,
    pub table_type: TableType,
    pub is_multiple: bool,
}

impl Field for BasicField {
    fn name(&self) -> &str {
        &self.name
    }

    fn table_type(&self) -> TableType {
        self.table_type
    }

    fn is_multiple(&self) -> bool {
        self.is_multiple
    }
}
