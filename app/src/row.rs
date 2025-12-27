use iced::widget::{button, column, row, text};
use iced::{Element, Length};
use iced::widget::image; // Import module/function to avoid ambiguity in list
use calibre_db::Book;

// Note: We need to know what Message to emit.
// Ideally, `Message` in main.rs should have variants like `EditBook(i32)` and `DeleteBook(i32)`.
// But row.rs doesn't know about `Message` enum in main.rs unless we define a trait or use a callback.
// The simplest way in Iced is to pass a function that creates the message.

pub fn view<'a, Message>(
    book: &Book, 
    cover: Option<image::Handle>,
    on_edit: impl Fn(i32) -> Message + 'a,
    on_delete: impl Fn(i32) -> Message + 'a,
) -> Element<'a, Message> 
where 
    Message: Clone + 'a
{
    let title = text(&book.title).size(18);
    let author = text(book.author_sort.as_deref().unwrap_or("Unknown")).size(14).style(iced::theme::Text::Color(iced::Color::from_rgb(0.5, 0.5, 0.5)));

    let cover_image: Element<'a, Message> = if let Some(handle) = cover {
        image(handle)
            .width(Length::Fixed(50.0))
            .height(Length::Fixed(75.0))
            .into()
    } else {
        // Placeholder or empty space
        iced::widget::container(text("No Cover").size(10))
            .width(Length::Fixed(50.0))
            .height(Length::Fixed(75.0))
            .center_x()
            .center_y()
            .into()
    };

    let details = column![title, author].spacing(5).width(Length::Fill);

    let actions = row![
        button("Edit").on_press(on_edit(book.id)).padding(5),
        button("Delete").on_press(on_delete(book.id)).padding(5).style(iced::theme::Button::Destructive),
    ].spacing(10);

    row![
        cover_image,
        details,
        actions
    ]
    .spacing(10)
    .padding(10)
    .width(Length::Fill)
    .align_items(iced::Alignment::Center)
    .into()
}

