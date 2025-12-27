
#[derive(Debug, Clone)]
pub struct Book {
    pub id: i32,
    pub title: String,
    pub sort: Option<String>,
    pub timestamp: Option<String>, // TODO: Parse to DateTime
    pub pubdate: Option<String>,   // TODO: Parse to DateTime
    pub series_index: f64,
    pub author_sort: Option<String>,
    pub isbn: Option<String>,
    pub lccn: Option<String>,
    pub path: String,
    pub has_cover: bool,
    pub uuid: Option<String>,
}
