use iced::widget::{column, container, scrollable, text, row, text_input, button};
use iced::{executor, Application, Command, Element, Length, Settings, Theme};
use calibre_db::{Book, Library};
use calibre_ebooks::epub::read_epub_metadata;
use calibre_utils::logging;
use std::path::PathBuf;

mod row;

pub fn main() -> iced::Result {
    logging::init();
    CalibreApp::run(Settings::default())
}

struct CalibreApp {
    library: Option<Library>,
    books: Vec<Book>,
    error_message: Option<String>,
    add_path_input: String,
    // Edit state
    editing_book_id: Option<i32>,
    edit_title_input: String,
    edit_author_input: String,
}

#[derive(Debug, Clone)]
enum Message {
    Loaded(Result<Vec<Book>, String>),
    AddPathChanged(String),
    AddBook,
    BookAdded(Result<i32, String>),
    DeleteBook(i32),
    EditBook(i32),
    EditTitleChanged(String),
    EditAuthorChanged(String),
    SaveEdit,
    CancelEdit,
    BookDeleted(Result<(), String>),
    BookUpdated(Result<(), String>),
}

impl Application for CalibreApp {
    type Executor = executor::Default;
    type Message = Message;
    type Theme = Theme;
    type Flags = ();

    fn new(_flags: ()) -> (Self, Command<Message>) {
        // Hardcoded path for dev/demo purposes
        let path = PathBuf::from("old_src/src/calibre/db/tests");
        
        // Initial load
        let (library, cmd) = match Library::open(path.clone()) {
            Ok(lib) => {
                // Determine books
                match lib.list_books() {
                    Ok(books) => (Some(lib), Command::perform(async { Ok(books) }, Message::Loaded)),
                    Err(e) => (Some(lib), Command::perform(async move { Err(e.to_string()) }, Message::Loaded)),
                }
            }
            Err(e) => (None, Command::perform(async move { Err(format!("Failed to open library at {:?}: {}", path, e)) }, Message::Loaded)),
        };

        (
            CalibreApp {
                library,
                books: vec![],
                error_message: None,
                add_path_input: String::new(),
                editing_book_id: None,
                edit_title_input: String::new(),
                edit_author_input: String::new(),
            },
            cmd,
        )
    }

    fn title(&self) -> String {
        String::from("Calibre Oxide")
    }

    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::Loaded(Ok(books)) => {
                self.books = books;
                self.error_message = None;
            }
            Message::Loaded(Err(e)) => {
                self.error_message = Some(e);
            }
            Message::AddPathChanged(val) => {
                self.add_path_input = val;
            }
            Message::AddBook => {
                if let Some(lib) = self.library.as_mut() {
                    let path_str = self.add_path_input.clone();
                    // Sync add for now
                    let path = PathBuf::from(&path_str);
                    if !path.exists() {
                         self.error_message = Some(format!("File not found: {:?}", path));
                         return Command::none();
                    }
                    
                    match read_epub_metadata(&path) {
                        Ok(meta) => {
                            match lib.add_book(&path, &meta) {
                                Ok(_) => {
                                    self.add_path_input.clear();
                                    // Reload books
                                    match lib.list_books() {
                                         Ok(books) => { self.books = books; self.error_message = None; }
                                         Err(e) => self.error_message = Some(e.to_string()),
                                    }
                                }
                                Err(e) => self.error_message = Some(format!("Failed to add book: {}", e)),
                            }
                        }
                        Err(e) => self.error_message = Some(format!("Failed to parse metadata: {}", e)),
                    }
                }
            }
            Message::BookAdded(_) => {} 
            Message::DeleteBook(id) => {
                if let Some(lib) = self.library.as_mut() {
                    match lib.delete_book(id) {
                         Ok(_) => {
                            // Reload
                            match lib.list_books() {
                                 Ok(books) => { self.books = books; self.error_message = None; }
                                 Err(e) => self.error_message = Some(e.to_string()),
                            }
                         }
                         Err(e) => self.error_message = Some(format!("Failed to delete: {}", e)),
                    }
                }
            }
            Message::EditBook(id) => {
                self.editing_book_id = Some(id);
                if let Some(book) = self.books.iter().find(|b| b.id == id) {
                    self.edit_title_input = book.title.clone();
                    self.edit_author_input = book.author_sort.clone().unwrap_or_default();
                }
            }
            Message::EditTitleChanged(val) => {
                self.edit_title_input = val;
            }
            Message::EditAuthorChanged(val) => {
                self.edit_author_input = val;
            }
            Message::CancelEdit => {
                self.editing_book_id = None;
                self.edit_title_input.clear();
                self.edit_author_input.clear();
            }
            Message::SaveEdit => {
                if let Some(id) = self.editing_book_id {
                    if let Some(lib) = self.library.as_mut() {
                        match lib.update_book_metadata(id, &self.edit_title_input, &self.edit_author_input) {
                            Ok(_) => {
                                self.editing_book_id = None;
                                self.edit_title_input.clear();
                                self.edit_author_input.clear();
                                // Reload
                                match lib.list_books() {
                                     Ok(books) => { self.books = books; self.error_message = None; }
                                     Err(e) => self.error_message = Some(e.to_string()),
                                }
                            }
                            Err(e) => self.error_message = Some(format!("Failed to update: {}", e)),
                        }
                    }
                }
            }
            Message::BookDeleted(_) => {},
            Message::BookUpdated(_) => {},
        }
        Command::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let content = if self.library.is_none() {
             if let Some(error) = &self.error_message {
                container(text(error).size(20)).into()
             } else {
                 container(text("Initializing...").size(20)).into()
             }
        } else {
            // Edit Modal Overlay Logic (Simulated with simple conditional rendering for now)
            if self.editing_book_id.is_some() {
                 let edit_form = column![
                    text("Edit Metadata").size(30),
                    text_input("Title", &self.edit_title_input).on_input(Message::EditTitleChanged).padding(10),
                    text_input("Author", &self.edit_author_input).on_input(Message::EditAuthorChanged).padding(10),
                    row![
                        button("Save").on_press(Message::SaveEdit).padding(10),
                        button("Cancel").on_press(Message::CancelEdit).padding(10).style(iced::theme::Button::Secondary),
                    ].spacing(20)
                 ].spacing(20).padding(20);

                 container(edit_form)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .center_x()
                    .center_y()
                    .into()
            } else {
                let book_list: Element<_> = if self.books.is_empty() {
                    text("No books found").into()
                } else {
                    let rows: Vec<_> = self.books.iter()
                        .map(|book| {
                            let cover = self.library.as_ref()
                                .and_then(|lib| lib.get_cover_path(book))
                                .map(iced::widget::image::Handle::from_path);
                            // Pass callbacks for actions
                            row::view(book, cover, Message::EditBook, Message::DeleteBook)
                        })
                        .collect();
                    
                    scrollable(column(rows).spacing(10))
                        .height(Length::Fill)
                        .into()
                };

                let add_book_row = row![
                    text_input("Path to EPUB", &self.add_path_input)
                        .on_input(Message::AddPathChanged)
                        .on_submit(Message::AddBook)
                        .padding(10),
                    button("Add Book").on_press(Message::AddBook).padding(10)
                ].spacing(10);
                
                let err_text = if let Some(e) = &self.error_message {
                    text(e).style(iced::theme::Text::Color(iced::Color::from_rgb(1.0, 0.0, 0.0)))
                } else {
                    text("")
                };

                container(
                    column![
                        text("Library").size(30),
                        err_text,
                        add_book_row,
                        book_list
                    ]
                    .spacing(20)
                    .padding(20)
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
            }
        };

        content
    }
}
