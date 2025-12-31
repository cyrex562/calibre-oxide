use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TableType {
    OneOne,
    ManyOne,
    ManyMany,
}

pub trait Table {
    fn name(&self) -> &str;
    fn table_type(&self) -> TableType;

    // In Python this was metadata dict. In Rust we might want a struct or just accessors.
    // For now, let's keep it simple.
}

#[derive(Debug, Clone)]
pub struct TableMetadata {
    pub table: String,
    pub column: String,
    pub datatype: String,
    pub is_multiple: bool,
    pub link_column: String,
}

// --- Specific Tables ---

pub struct AuthorsTable {
    metadata: TableMetadata,
    // id_map, link_map, etc. will be needed.
    // In python these were loaded dynamically.
    // For this sprint we are defining structure.
    pub id_map: HashMap<i32, String>,
    pub link_map: HashMap<i32, String>,
    pub asort_map: HashMap<i32, String>,
    pub col_book_map: HashMap<i32, HashSet<i32>>,
    pub book_col_map: HashMap<i32, Vec<i32>>,
}

impl AuthorsTable {
    pub fn new() -> Self {
        AuthorsTable {
            metadata: TableMetadata {
                table: "authors".to_string(),
                column: "name".to_string(),
                datatype: "text".to_string(),
                is_multiple: true,
                link_column: "author".to_string(),
            },
            id_map: HashMap::new(),
            link_map: HashMap::new(),
            asort_map: HashMap::new(),
            col_book_map: HashMap::new(),
            book_col_map: HashMap::new(),
        }
    }
}

impl Table for AuthorsTable {
    fn name(&self) -> &str {
        "authors"
    }

    fn table_type(&self) -> TableType {
        TableType::ManyMany
    }
}

pub struct FormatsTable {
    // metadata: TableMetadata,
    pub fname_map: HashMap<i32, HashMap<String, String>>, // book_id -> {fmt -> name}
    pub size_map: HashMap<i32, HashMap<String, u64>>,
    pub col_book_map: HashMap<String, HashSet<i32>>, // fmt -> {book_id}
    pub book_col_map: HashMap<i32, Vec<String>>,     // book_id -> [fmt]
}

impl FormatsTable {
    pub fn new() -> Self {
        FormatsTable {
            /*
            metadata: TableMetadata {
                table: "data".to_string(),
                column: "format".to_string(), // logic differs slightly for formats
                datatype: "text".to_string(),
                is_multiple: true,
                link_column: "book".to_string(),
            },
            */
            fname_map: HashMap::new(),
            size_map: HashMap::new(),
            col_book_map: HashMap::new(),
            book_col_map: HashMap::new(),
        }
    }
}

impl Table for FormatsTable {
    fn name(&self) -> &str {
        "formats"
    }

    fn table_type(&self) -> TableType {
        TableType::ManyMany
    }
}
