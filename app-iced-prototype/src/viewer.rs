use std::thread;
use wry::application::{
    event::{Event, StartCause, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};
use wry::webview::WebViewBuilder;

pub fn open_viewer(url: String) {
    // Spawning in a thread to avoid blocking the main Iced loop.
    // WARNING: On macOS this will fail (EventLoop must be on main thread).
    // On Windows/Linux it might work or fail depending on the backend.
    // For a robust cross-platform solution we might need a separate process or Iced integration.
    thread::spawn(move || {
        let event_loop = EventLoop::new();
        let window = WindowBuilder::new()
            .with_title("Calibre-Oxide Viewer")
            .build(&event_loop)
            .unwrap();

        let _webview = WebViewBuilder::new(window)
            .unwrap()
            .with_url(&url)
            .unwrap()
            .build()
            .unwrap();

        event_loop.run(move |event, _, control_flow| {
            *control_flow = ControlFlow::Wait;

            match event {
                Event::WindowEvent {
                    event: WindowEvent::CloseRequested,
                    ..
                } => *control_flow = ControlFlow::Exit,
                _ => (),
            }
        });
    });
}
