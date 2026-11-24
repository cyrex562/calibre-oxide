use console::{Term, Style};

pub fn geometry() -> (u16, u16) {
    let term = Term::stdout();
    term.size()
}

pub fn colored(text: &str, fg: Option<&str>, bg: Option<&str>, bold: bool) -> String {
    let mut style = Style::new();
    
    if let Some(c) = fg {
        match c {
            "red" => style = style.red(),
            "yellow" => style = style.yellow(),
            "green" => style = style.green(),
            "blue" => style = style.blue(),
            "cyan" => style = style.cyan(),
            "magenta" => style = style.magenta(),
            "white" => style = style.white(),
            // Add others as needed
            _ => {}
        }
    }
    
    if let Some(c) = bg {
        match c {
             "red" => style = style.on_red(),
             "yellow" => style = style.on_yellow(),
             "green" => style = style.on_green(),
             "blue" => style = style.on_blue(),
             "cyan" => style = style.on_cyan(),
             "magenta" => style = style.on_magenta(),
             "white" => style = style.on_white(),
             _ => {}
        }
    }
    
    if bold {
        style = style.bold();
    }
    
    style.apply_to(text).to_string()
}
