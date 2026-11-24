#[derive(Debug, Clone, PartialEq)]
pub enum PathCommand {
    MoveTo(f64, f64),
    LineTo(f64, f64),
    CubicTo(f64, f64, f64, f64, f64, f64),
    QuadTo(f64, f64, f64, f64),
    Close,
}

pub struct SvgPathParser<'a> {
    pos: usize,
    data: &'a [u8],
}

impl<'a> SvgPathParser<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        SvgPathParser { pos: 0, data }
    }

    fn read_byte(&mut self) -> Option<u8> {
        if self.pos < self.data.len() {
            let b = self.data[self.pos];
            self.pos += 1;
            Some(b)
        } else {
            None
        }
    }

    fn peek_byte(&self) -> Option<u8> {
        if self.pos < self.data.len() {
            Some(self.data[self.pos])
        } else {
            None
        }
    }
    
    fn skip_separators(&mut self) {
         while let Some(b) = self.peek_byte() {
             if b == b',' || b == b' ' || b == b'\n' || b == b'\t' || b == b'\r' {
                 self.pos += 1;
             } else {
                 break;
             }
         }
    }

    fn parse_float(&mut self) -> Result<f64, &'static str> {
        self.skip_separators();
        let start = self.pos;
        let mut has_dot = false;
        let mut has_exp = false;
        
        // Manual float parsing structure to match SVG rules loosely
        while let Some(b) = self.peek_byte() {
            match b {
                b'+' | b'-' => {
                     // Sign only allowed at start or after 'e'
                     if self.pos > start {
                         // Check if previous char was 'e' or 'E'
                         let prev = self.data[self.pos - 1];
                         if prev != b'e' && prev != b'E' {
                             break;
                         }
                     }
                     self.pos += 1;
                },
                b'0'..=b'9' => { self.pos += 1; },
                b'.' => {
                    if has_dot { break; } // Second dot starts new number?
                    // SVG path data: "0.5.5" -> 0.5, 0.5
                    has_dot = true;
                    self.pos += 1;
                },
                b'e' | b'E' => {
                    if has_exp { break; }
                    has_exp = true;
                    self.pos += 1;
                },
                _ => break,
            }
        }
        
        if self.pos == start {
            return Err("Expected float");
        }
        
        let s = std::str::from_utf8(&self.data[start..self.pos]).map_err(|_| "Invalid UTF8")?;
        s.parse::<f64>().map_err(|_| "Invalid float")
    }
}

pub fn parse_svg_path(d: &str) -> Result<Vec<PathCommand>, String> {
    let mut commands = Vec::new();
    let data = d.as_bytes();
    let mut parser = SvgPathParser::new(data);
    
    let mut last_cmd = 0u8;
    
    // State
    let mut x = 0.0;
    let mut y = 0.0;
    
    // For reflection (S, T)
    let mut last_control_x = 0.0;
    let mut last_control_y = 0.0;

    parser.skip_separators();

    while let Some(mut cmd) = parser.read_byte() {
        // Handle repeated commands implicit in numbers?
        // If cmd is digit or sign or dot, it's a parameter for `last_cmd`.
        let is_param = match cmd {
            b'0'..=b'9' | b'-' | b'+' | b'.' => true,
            _ => false,
        };
        
        if is_param {
            parser.pos -= 1; // rewind
            if last_cmd == b'M' { cmd = b'L'; }
            else if last_cmd == b'm' { cmd = b'l'; }
            else if last_cmd == 0 { return Err("Number without command".to_string()); }
            else { cmd = last_cmd; } // Repeat last command
        }
        
        // Reset control reflection if not a curve
        match cmd {
             b'C' | b'c' | b'S' | b's' | b'Q' | b'q' | b'T' | b't' => {},
             _ => { last_control_x = x; last_control_y = y; }
        }

        match cmd {
            b'M' => { // MoveTo Abs
                x = parser.parse_float().map_err(|e| e.to_string())?;
                y = parser.parse_float().map_err(|e| e.to_string())?;
                commands.push(PathCommand::MoveTo(x, y));
                last_control_x = x; last_control_y = y;
            },
            b'm' => { // MoveTo Rel
                let dx = parser.parse_float().map_err(|e| e.to_string())?;
                let dy = parser.parse_float().map_err(|e| e.to_string())?;
                x += dx; y += dy;
                commands.push(PathCommand::MoveTo(x, y));
                last_control_x = x; last_control_y = y;
            },
            b'Z' | b'z' => {
                commands.push(PathCommand::Close);
                // Should close loop logic?
            },
            b'L' => {
                x = parser.parse_float().map_err(|e| e.to_string())?;
                y = parser.parse_float().map_err(|e| e.to_string())?;
                commands.push(PathCommand::LineTo(x, y));
            },
            b'l' => {
                x += parser.parse_float().map_err(|e| e.to_string())?;
                y += parser.parse_float().map_err(|e| e.to_string())?;
                commands.push(PathCommand::LineTo(x, y));
            },
            b'H' => {
                x = parser.parse_float().map_err(|e| e.to_string())?;
                commands.push(PathCommand::LineTo(x, y));
            },
            b'h' => {
                x += parser.parse_float().map_err(|e| e.to_string())?;
                commands.push(PathCommand::LineTo(x, y));
            },
            b'V' => {
                y = parser.parse_float().map_err(|e| e.to_string())?;
                commands.push(PathCommand::LineTo(x, y));
            },
            b'v' => {
                y += parser.parse_float().map_err(|e| e.to_string())?;
                commands.push(PathCommand::LineTo(x, y));
            },
            b'C' => {
                let cx1 = parser.parse_float().map_err(|e| e.to_string())?;
                let cy1 = parser.parse_float().map_err(|e| e.to_string())?;
                let x2 = parser.parse_float().map_err(|e| e.to_string())?;
                let y2 = parser.parse_float().map_err(|e| e.to_string())?;
                x = parser.parse_float().map_err(|e| e.to_string())?;
                y = parser.parse_float().map_err(|e| e.to_string())?;
                commands.push(PathCommand::CubicTo(cx1, cy1, x2, y2, x, y));
                last_control_x = x2; last_control_y = y2;
            },
            b'c' => {
                let cx1 = x + parser.parse_float().map_err(|e| e.to_string())?;
                let cy1 = y + parser.parse_float().map_err(|e| e.to_string())?;
                let x2 = x + parser.parse_float().map_err(|e| e.to_string())?;
                let y2 = y + parser.parse_float().map_err(|e| e.to_string())?;
                x += parser.parse_float().map_err(|e| e.to_string())?;
                y += parser.parse_float().map_err(|e| e.to_string())?;
                commands.push(PathCommand::CubicTo(cx1, cy1, x2, y2, x, y));
                last_control_x = x2; last_control_y = y2;
            },
            b'S' | b's' => {
                // To implement S/s, we need last control point.
                // Reflection: 
                 let cx1 = if last_cmd == b'C' || last_cmd == b'c' || last_cmd == b'S' || last_cmd == b's' {
                     2.0 * x - last_control_x
                 } else { x };
                 let cy1 = if last_cmd == b'C' || last_cmd == b'c' || last_cmd == b'S' || last_cmd == b's' {
                     2.0 * y - last_control_y
                 } else { y };
                 
                 let (dx2, dy2, dx, dy) = if cmd == b'S' {
                     (
                       parser.parse_float().map_err(|e| e.to_string())?,
                       parser.parse_float().map_err(|e| e.to_string())?,
                       parser.parse_float().map_err(|e| e.to_string())?,
                       parser.parse_float().map_err(|e| e.to_string())?
                     )
                 } else {
                     (
                       x + parser.parse_float().map_err(|e| e.to_string())?,
                       y + parser.parse_float().map_err(|e| e.to_string())?,
                       x + parser.parse_float().map_err(|e| e.to_string())?,
                       y + parser.parse_float().map_err(|e| e.to_string())?
                     )
                 };
                 
                 let x2 = dx2; let y2 = dy2;
                 x = dx; y = dy;
                 commands.push(PathCommand::CubicTo(cx1, cy1, x2, y2, x, y));
                 last_control_x = x2; last_control_y = y2;
            },
            
            // Q, q, T, t (Quadratics) - map to Cubic? Or emit Quad?
            // Python code mapped to Cubic in QPainterPath.
            // I'll emit Quad for definitions.
             b'Q' => {
                let cx1 = parser.parse_float().map_err(|e| e.to_string())?;
                let cy1 = parser.parse_float().map_err(|e| e.to_string())?;
                x = parser.parse_float().map_err(|e| e.to_string())?;
                y = parser.parse_float().map_err(|e| e.to_string())?;
                commands.push(PathCommand::QuadTo(cx1, cy1, x, y));
                last_control_x = cx1; last_control_y = cy1;
            },
             b'q' => {
                let cx1 = x + parser.parse_float().map_err(|e| e.to_string())?;
                let cy1 = y + parser.parse_float().map_err(|e| e.to_string())?;
                x += parser.parse_float().map_err(|e| e.to_string())?;
                y += parser.parse_float().map_err(|e| e.to_string())?;
                commands.push(PathCommand::QuadTo(cx1, cy1, x, y));
                last_control_x = cx1; last_control_y = cy1;
            },
            
            _ => return Err(format!("Unknown command {}", cmd as char)),
        }
        
        last_cmd = cmd;
        parser.skip_separators();
    }
    
    Ok(commands)
}
