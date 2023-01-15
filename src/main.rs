use std::{
    fmt,
    io::{stdout, Write},
    ops::Range,
    path::{Path, PathBuf},
};

use anyhow::Result;
use clap::Parser;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    style::Print,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use euclid::{Point2D, Size2D};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

struct U;
type Size = Size2D<usize, U>;
type Point = Point2D<usize, U>;

#[derive(Debug, Parser)]
#[clap(name = env!("CARGO_PKG_NAME"), version, author, about)]
struct Opts {
    path: PathBuf,
}

fn is_linebreak(str: &str) -> bool {
    matches!(
        str,
        "\r\n"
            | "\u{000A}"
            | "\u{000B}"
            | "\u{000C}"
            | "\u{000D}"
            | "\u{0085}"
            | "\u{2028}"
            | "\u{2029}"
    )
}

#[allow(dead_code)]
#[allow(clippy::type_complexity)]
fn rect(
    buf: &Buffer,
    off: usize,
    scr: Size,
    fun: fn(Point, Point, usize, &Range<usize>, &Range<usize>, &[u8]) -> i32,
) {
    let mut phy = Point::new(0, 0);

    let mut col = 0;
    'outer: for (y, line) in buf.line.iter().enumerate().skip(off) {
        // special case
        if col == scr.width + 1 {
            phy.y -= 1;
        }
        col = line.cols;
        for (x, (bin, seg)) in line.span().enumerate() {
            let log = Point::new(x, y);
            match fun(phy, log, col, &bin, &seg, &line.data) {
                1 => break,
                2 => break 'outer,
                3 => continue,
                _ => {}
            }
            phy.x += seg.len();
            if phy.x >= scr.width {
                phy.x = 0;
                phy.y += 1;
                if phy.y >= scr.height {
                    break;
                }
            }
        }
        phy.x = 0;
        phy.y += 1;
        if phy.y >= scr.height {
            break;
        }
    }
}

#[derive(Debug, PartialEq, Clone, Copy)]
enum Action {
    Full,
    Up,
    Down,
    Left,
    Right,
    Resize(Size),
}

#[derive(Debug, Default)]
struct LineBr {
    data: Vec<u8>,
    span: Vec<(u8, u8)>,
    cols: usize,
}

impl LineBr {
    fn span(&self) -> impl Iterator<Item = (Range<usize>, Range<usize>)> + '_ {
        self.span.iter().scan((0..0, 0..0), |item, next| {
            item.0 = item.0.end..item.0.end + next.0 as usize;
            item.1 = item.1.end..item.1.end + next.1 as usize;
            Some(item.clone())
        })
    }
}

#[derive(Debug)]
struct Buffer {
    line: Vec<LineBr>,
}

impl Buffer {
    fn rows(&self) -> usize {
        self.line.len()
    }

    fn cols(&self, row: usize) -> usize {
        self.line[row].cols
    }
}

impl Default for Buffer {
    fn default() -> Self {
        Buffer::from("")
    }
}

impl From<&str> for Buffer {
    fn from(text: &str) -> Self {
        let mut line: Vec<LineBr> = Default::default();
        let mut last;
        line.push(Default::default());
        last = unsafe { line.last_mut().unwrap_unchecked() };
        for segm in text.graphemes(true) {
            last.data.extend(segm.as_bytes());
            if !is_linebreak(segm) {
                last.span.push((segm.len() as _, segm.width() as _));
                last.cols += segm.width();
            } else {
                last.span.push((segm.len() as _, 1));
                last.cols += 1;
                line.push(Default::default());
                last = unsafe { line.last_mut().unwrap_unchecked() };
            }
        }
        last.span.push((0, 1));
        last.cols += 1;
        Self { line }
    }
}

impl fmt::Display for Buffer {
    fn fmt(&self, file: &mut fmt::Formatter<'_>) -> fmt::Result {
        for line in &self.line {
            write!(file, "{}", unsafe {
                std::str::from_utf8_unchecked(&line.data)
            })?;
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
struct Screen;

impl Screen {
    fn init() -> Result<()> {
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            _ = Screen::fini();
            hook(info);
        }));
        terminal::enable_raw_mode()?;
        execute!(stdout(), EnterAlternateScreen)?;
        Ok(())
    }

    fn fini() -> Result<()> {
        execute!(stdout(), LeaveAlternateScreen)?;
        terminal::disable_raw_mode()?;
        Ok(())
    }

    fn size() -> Result<Size> {
        let (cols, rows) = terminal::size()?;
        Ok(Size::new(cols as _, rows as _))
    }
}

#[derive(Debug, Default)]
struct Editor {
    offset: usize,
    buffer: Buffer,
    cursor: Point,
    scrsiz: Size,
}

impl Editor {
    fn load(&mut self, path: &Path) -> Result<()> {
        self.buffer = Buffer::from(std::fs::read_to_string(path)?.as_str());
        Ok(())
    }

    fn save(&mut self, path: &Path) -> Result<()> {
        write!(std::fs::File::create(path)?, "{}", &self.buffer)?;
        Ok(())
    }

    fn draw(&mut self, action: Vec<Action>) -> Result<()> {
        if action.is_empty() {
            return Ok(());
        }
        let action = action[0];

        let mut all = action == Action::Full;

        if let Action::Resize(scrsiz) = action {
            if self.scrsiz != scrsiz {
                self.scrsiz = scrsiz;
                all = true;
            }
        }

        match action {
            Action::Up if 0 < self.cursor.y => self.cursor.y -= 1,
            Action::Down if self.cursor.y + 1 < self.buffer.rows() => self.cursor.y += 1,
            Action::Right | Action::Left => {
                self.cursor.x = self.cursor.x.min(self.buffer.cols(self.cursor.y) - 1)
            }
            _ => {}
        }

        // Step1: cursor
        let mut offset = self.offset.min(self.cursor.y);
        if self.cursor.y + 1 >= self.scrsiz.height {
            offset = offset.max(self.cursor.y + 1 - self.scrsiz.height);
        }
        let cur = 'outer: loop {
            let mut pos = Point::new(0, 0);

            // special case
            let mut col_pre = 0;

            for (l, lbr) in self.buffer.line.iter().enumerate().skip(offset) {
                let mut fix = false;
                let mut sst_pre = 0;
                let mut pos_pre = pos;

                // special case
                if col_pre == self.scrsiz.width + 1 {
                    pos.y -= 1;
                }

                for (c, (_, seg)) in lbr.span().enumerate() {
                    // skip 0-width segment
                    if seg.is_empty() {
                        continue;
                    }

                    if l == self.cursor.y && seg.contains(&self.cursor.x) {
                        match action {
                            Action::Right if !fix && self.cursor.x + 1 < lbr.cols => {
                                self.cursor.x = seg.end;
                                fix = true; // cur will be determined by the next iteration
                            }
                            Action::Left if 0 < self.cursor.x => {
                                self.cursor.x = sst_pre;
                                break 'outer pos_pre;
                            }
                            _ => break 'outer pos,
                        }
                    }

                    if c == lbr.span.len() - 1 {
                        break;
                    }

                    // save for Action::Left
                    pos_pre = pos;
                    sst_pre = seg.start;

                    pos.x += seg.len();
                    if pos.x >= self.scrsiz.width {
                        pos.x = 0;
                        pos.y += 1;
                        if pos.y >= self.scrsiz.height {
                            break;
                        }
                    }
                }

                if l == self.cursor.y && matches!(action, Action::Up | Action::Down) {
                    break 'outer pos;
                }

                pos.x = 0;
                pos.y += 1;
                if pos.y >= self.scrsiz.height {
                    break;
                }

                // special case
                col_pre = lbr.cols;
            }
            offset += 1;
            all = true; // need for line-wrapped text
        };
        if self.offset != offset {
            self.offset = offset;
            all = true;
        }

        if !all {
            execute!(stdout(), cursor::MoveTo(cur.x as _, cur.y as _),)?;
            return Ok(());
        }

        // Step2: output
        let out = {
            // special case
            let mut col_pre = 0;

            let mut out = Vec::<u8>::with_capacity(self.scrsiz.area() * 4);
            let mut pos = Point::new(0, 0);
            #[allow(unused_variables)]
            for (l, lbr) in self.buffer.line.iter().enumerate().skip(self.offset) {
                let mut eol = 0;

                // special case
                if col_pre == self.scrsiz.width + 1 {
                    pos.y -= 1;
                }

                for (c, (str, seg)) in lbr.span().enumerate() {
                    if c == lbr.span.len() - 1 {
                        break;
                    }

                    pos.x += seg.len();
                    if pos.x >= self.scrsiz.width {
                        pos.x = 0;
                        pos.y += 1;
                        if pos.y >= self.scrsiz.height {
                            break;
                        }
                    }

                    eol = str.end;
                }

                out.extend(&lbr.data.as_slice()[..eol]);

                pos.x = 0;
                pos.y += 1;
                if pos.y >= self.scrsiz.height {
                    break;
                }

                out.extend(b"\r\n");

                // special case
                col_pre = lbr.cols;
            }
            out
        };

        execute!(
            stdout(),
            cursor::Hide,
            terminal::Clear(terminal::ClearType::All),
            cursor::MoveTo(0, 0),
            Print(unsafe { std::str::from_utf8_unchecked(&out) }),
            cursor::MoveTo(cur.x as _, cur.y as _),
            cursor::Show
        )?;
        Ok(())
    }
}

fn main() -> Result<()> {
    let opts = Opts::parse();
    let path = &opts.path;

    let mut editor = Editor::default();

    if path.exists() {
        editor.load(path)?;
    }
    Screen::init()?;
    editor.draw(vec![Action::Resize(Screen::size().unwrap())])?;
    loop {
        let mut action = vec![];
        #[allow(clippy::single_match)]
        #[allow(clippy::collapsible_match)]
        match event::read()? {
            Event::Key(event) => match event {
                KeyEvent {
                    modifiers: KeyModifiers::CONTROL,
                    code,
                    ..
                } => match code {
                    KeyCode::Char('c') => break,
                    KeyCode::Char('s') => editor.save(path)?,
                    _ => {}
                },
                KeyEvent {
                    modifiers: KeyModifiers::NONE,
                    code,
                    ..
                } => match code {
                    KeyCode::Up => action.push(Action::Up),
                    KeyCode::Down => action.push(Action::Down),
                    KeyCode::Left => action.push(Action::Left),
                    KeyCode::Right => action.push(Action::Right),
                    _ => action.push(Action::Full),
                },
                _ => {}
            },
            Event::Resize(cols, rows) => {
                action.push(Action::Resize(Size::new(cols as _, rows as _)))
            }
            _ => {}
        };
        editor.draw(action)?;
    }
    Screen::fini()?;
    Ok(())
}

#[cfg(test)]
include!("test.rs");
