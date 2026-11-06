//! A small Markdown renderer for egui, built on pulldown-cmark: headings, paragraphs, bold /
//! italic / strikethrough / inline code, links, bullet and numbered lists, block quotes, tables,
//! rules and fenced code blocks with a **Copy** button.

use eframe::egui::{self, FontId, RichText, TextStyle, Ui};
use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

#[derive(Clone, Debug, PartialEq)]
enum Inline {
    Text { text: String, strong: bool, italic: bool, strike: bool, code: bool, heading: Option<u8> },
    Link { text: String, url: String },
    Break,
}

#[derive(Clone, Debug, PartialEq)]
enum Block {
    Paragraph(Vec<Inline>),
    Heading(u8, Vec<Inline>),
    Code { lang: String, code: String },
    List { start: Option<u64>, items: Vec<Vec<Block>> },
    Quote(Vec<Block>),
    Table { align: Vec<Alignment>, rows: Vec<Vec<Vec<Inline>>> },
    Rule,
}

// ------------------------------------------------------------------ parsing

#[derive(Default)]
struct Style {
    strong: u32,
    italic: u32,
    strike: u32,
    heading: Option<u8>,
}

struct Builder<'a> {
    events: std::iter::Peekable<Parser<'a>>,
}

impl<'a> Builder<'a> {
    fn blocks_until(&mut self, end: Option<TagEnd>) -> Vec<Block> {
        let mut out = Vec::new();
        // loose text (e.g. tight list items) collects here
        let mut loose: Vec<Inline> = Vec::new();
        let mut style = Style::default();
        while let Some(ev) = self.events.next() {
            match ev {
                Event::End(e) if Some(e) == end => break,
                Event::Start(Tag::Paragraph) => {
                    flush(&mut loose, &mut out);
                    out.push(Block::Paragraph(self.inlines_until(TagEnd::Paragraph, Style::default())));
                }
                Event::Start(Tag::Heading { level, .. }) => {
                    flush(&mut loose, &mut out);
                    let n = heading_num(level);
                    let st = Style { heading: Some(n), ..Default::default() };
                    out.push(Block::Heading(n, self.inlines_until(TagEnd::Heading(level), st)));
                }
                Event::Start(Tag::CodeBlock(kind)) => {
                    flush(&mut loose, &mut out);
                    let lang = match kind {
                        CodeBlockKind::Fenced(l) => l.split_whitespace().next().unwrap_or("").to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    let mut code = String::new();
                    for ev in self.events.by_ref() {
                        match ev {
                            Event::Text(t) => code.push_str(&t),
                            Event::End(TagEnd::CodeBlock) => break,
                            _ => {}
                        }
                    }
                    if code.ends_with('\n') {
                        code.pop();
                    }
                    out.push(Block::Code { lang, code });
                }
                Event::Start(Tag::List(start)) => {
                    flush(&mut loose, &mut out);
                    let mut items = Vec::new();
                    loop {
                        match self.events.next() {
                            Some(Event::Start(Tag::Item)) => items.push(self.blocks_until(Some(TagEnd::Item))),
                            Some(Event::End(TagEnd::List(_))) | None => break,
                            _ => {}
                        }
                    }
                    out.push(Block::List { start, items });
                }
                Event::Start(Tag::BlockQuote(_)) => {
                    flush(&mut loose, &mut out);
                    out.push(Block::Quote(self.blocks_until(Some(TagEnd::BlockQuote(None)))));
                }
                Event::Start(Tag::Table(align)) => {
                    flush(&mut loose, &mut out);
                    out.push(self.table(align));
                }
                Event::Rule => {
                    flush(&mut loose, &mut out);
                    out.push(Block::Rule);
                }
                Event::End(TagEnd::BlockQuote(_)) if matches!(end, Some(TagEnd::BlockQuote(_))) => break,
                other => self.inline_event(other, &mut style, &mut loose),
            }
        }
        flush(&mut loose, &mut out);
        out
    }

    fn table(&mut self, align: Vec<Alignment>) -> Block {
        let mut rows = Vec::new();
        let mut row: Vec<Vec<Inline>> = Vec::new();
        while let Some(ev) = self.events.next() {
            match ev {
                Event::Start(Tag::TableCell) => {
                    row.push(self.inlines_until(TagEnd::TableCell, Style::default()));
                }
                Event::End(TagEnd::TableHead) | Event::End(TagEnd::TableRow) => rows.push(std::mem::take(&mut row)),
                Event::End(TagEnd::Table) => break,
                _ => {}
            }
        }
        Block::Table { align, rows }
    }

    fn inlines_until(&mut self, end: TagEnd, mut style: Style) -> Vec<Inline> {
        let mut out = Vec::new();
        while let Some(ev) = self.events.next() {
            if let Event::End(e) = &ev {
                if *e == end {
                    break;
                }
            }
            self.inline_event(ev, &mut style, &mut out);
        }
        out
    }

    fn inline_event(&mut self, ev: Event<'a>, st: &mut Style, out: &mut Vec<Inline>) {
        let text = |t: String, st: &Style, code: bool| Inline::Text {
            text: t,
            strong: st.strong > 0,
            italic: st.italic > 0,
            strike: st.strike > 0,
            code,
            heading: st.heading,
        };
        match ev {
            Event::Text(t) => out.push(text(t.to_string(), st, false)),
            Event::Code(t) => out.push(text(t.to_string(), st, true)),
            Event::SoftBreak => out.push(text(" ".into(), st, false)),
            Event::HardBreak => out.push(Inline::Break),
            Event::Html(t) | Event::InlineHtml(t) => out.push(text(t.to_string(), st, false)),
            Event::TaskListMarker(done) => out.push(text(if done { "☑ ".into() } else { "☐ ".into() }, st, false)),
            Event::Start(Tag::Strong) => st.strong += 1,
            Event::End(TagEnd::Strong) => st.strong = st.strong.saturating_sub(1),
            Event::Start(Tag::Emphasis) => st.italic += 1,
            Event::End(TagEnd::Emphasis) => st.italic = st.italic.saturating_sub(1),
            Event::Start(Tag::Strikethrough) => st.strike += 1,
            Event::End(TagEnd::Strikethrough) => st.strike = st.strike.saturating_sub(1),
            Event::Start(Tag::Link { dest_url, .. }) => {
                let mut label = String::new();
                for ev in self.events.by_ref() {
                    match ev {
                        Event::Text(t) | Event::Code(t) => label.push_str(&t),
                        Event::End(TagEnd::Link) => break,
                        _ => {}
                    }
                }
                if label.is_empty() {
                    label = dest_url.to_string();
                }
                out.push(Inline::Link { text: label, url: dest_url.to_string() });
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                let mut alt = String::new();
                for ev in self.events.by_ref() {
                    match ev {
                        Event::Text(t) => alt.push_str(&t),
                        Event::End(TagEnd::Image) => break,
                        _ => {}
                    }
                }
                out.push(Inline::Link { text: format!("🖼 {}", if alt.is_empty() { "image" } else { &alt }), url: dest_url.to_string() });
            }
            _ => {}
        }
    }
}

/// Join neighbouring text pieces that look the same. The parser splits text at characters like
/// `[`, and every piece would otherwise become its own label (odd wrapping and spacing).
fn merge_text(items: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::with_capacity(items.len());
    for it in items {
        if let (
            Some(Inline::Text { text: prev, strong: s1, italic: i1, strike: k1, code: c1, heading: h1 }),
            Inline::Text { text, strong, italic, strike, code, heading },
        ) = (out.last_mut(), &it)
        {
            if *s1 == *strong && *i1 == *italic && *k1 == *strike && *c1 == *code && *h1 == *heading {
                prev.push_str(text);
                continue;
            }
        }
        out.push(it);
    }
    out
}

fn flush(loose: &mut Vec<Inline>, out: &mut Vec<Block>) {
    if !loose.is_empty() {
        out.push(Block::Paragraph(std::mem::take(loose)));
    }
}

fn heading_num(l: HeadingLevel) -> u8 {
    match l {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn parse(md: &str) -> Vec<Block> {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let blocks = Builder { events: Parser::new_ext(md, opts).peekable() }.blocks_until(None);
    blocks.into_iter().map(merge_block).collect()
}

fn merge_block(b: Block) -> Block {
    match b {
        Block::Paragraph(i) => Block::Paragraph(merge_text(i)),
        Block::Heading(l, i) => Block::Heading(l, merge_text(i)),
        Block::List { start, items } => Block::List { start, items: items.into_iter().map(|bs| bs.into_iter().map(merge_block).collect()).collect() },
        Block::Quote(bs) => Block::Quote(bs.into_iter().map(merge_block).collect()),
        Block::Table { align, rows } => Block::Table { align, rows: rows.into_iter().map(|r| r.into_iter().map(merge_text).collect()).collect() },
        other => other,
    }
}

// ------------------------------------------------------------------ drawing

/// Draw `md` into `ui`. `id` must be unique per message (it keys the code blocks' state).
pub fn show(ui: &mut Ui, md: &str, id: impl std::hash::Hash) {
    let blocks = parse(md);
    ui.push_id(id, |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        for (i, b) in blocks.iter().enumerate() {
            ui.push_id(i, |ui| block(ui, b));
        }
    });
}

fn block(ui: &mut Ui, b: &Block) {
    match b {
        Block::Paragraph(inl) => inlines(ui, inl),
        Block::Heading(_, inl) => {
            ui.add_space(4.0);
            inlines(ui, inl);
        }
        Block::Code { lang, code } => code_block(ui, lang, code),
        Block::List { start, items } => {
            for (n, item) in items.iter().enumerate() {
                ui.horizontal_top(|ui| {
                    let marker = match start {
                        Some(s) => format!("{}.", s + n as u64),
                        None => "•".to_string(),
                    };
                    let accent = crate::theme::of(ui).accent;
                    ui.add_sized([18.0, 0.0], egui::Label::new(RichText::new(marker).family(crate::theme::bold()).color(accent)));
                    ui.vertical(|ui| {
                        for (j, b) in item.iter().enumerate() {
                            ui.push_id((n, j), |ui| block(ui, b));
                        }
                    });
                });
            }
        }
        Block::Quote(inner) => {
            let color = crate::theme::of(ui).accent.gamma_multiply(0.7);
            egui::Frame::none()
                .stroke(egui::Stroke::NONE)
                .inner_margin(egui::Margin { left: 10.0, right: 4.0, top: 2.0, bottom: 2.0 })
                .show(ui, |ui| {
                    let r = ui.vertical(|ui| {
                        for (j, b) in inner.iter().enumerate() {
                            ui.push_id(j, |ui| block(ui, b));
                        }
                    });
                    let rect = r.response.rect;
                    ui.painter().line_segment(
                        [rect.left_top() - egui::vec2(8.0, 0.0), rect.left_bottom() - egui::vec2(8.0, 0.0)],
                        egui::Stroke::new(3.0_f32, color),
                    );
                });
        }
        Block::Table { align, rows } => {
            egui::ScrollArea::horizontal().show(ui, |ui| {
                egui::Grid::new("table").striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
                    for (r, row) in rows.iter().enumerate() {
                        for (c, cell) in row.iter().enumerate() {
                            let layout = match align.get(c) {
                                Some(Alignment::Right) => egui::Layout::right_to_left(egui::Align::Center),
                                Some(Alignment::Center) => egui::Layout::top_down(egui::Align::Center),
                                _ => egui::Layout::left_to_right(egui::Align::Center),
                            };
                            ui.with_layout(layout, |ui| {
                                if r == 0 {
                                    let bold: Vec<Inline> = cell
                                        .iter()
                                        .map(|i| match i {
                                            Inline::Text { text, italic, strike, code, heading, .. } => Inline::Text {
                                                text: text.clone(),
                                                strong: true,
                                                italic: *italic,
                                                strike: *strike,
                                                code: *code,
                                                heading: *heading,
                                            },
                                            other => other.clone(),
                                        })
                                        .collect();
                                    inlines_nowrap(ui, &bold);
                                } else {
                                    inlines_nowrap(ui, cell);
                                }
                            });
                        }
                        ui.end_row();
                    }
                });
            });
        }
        Block::Rule => {
            ui.separator();
        }
    }
}

fn inlines(ui: &mut Ui, items: &[Inline]) {
    ui.scope(|ui| {
        // A wrapped row starts out as tall as a button; make it as tall as a line of text so
        // the first line is not spaced differently from the others.
        let body = ui.style().text_styles.get(&TextStyle::Body).map(|f| f.size).unwrap_or(15.0);
        ui.spacing_mut().interact_size.y = (body * 1.5).round();
        ui.horizontal_wrapped(|ui| inline_items(ui, items, true));
    });
}

/// Table cells: no wrapping (the table scrolls sideways instead of squeezing the columns).
fn inlines_nowrap(ui: &mut Ui, items: &[Inline]) {
    ui.horizontal(|ui| inline_items(ui, items, false));
}

fn inline_items(ui: &mut Ui, items: &[Inline], wrap: bool) {
    {
        // One fixed line height for every piece, and no extra gap between wrapped rows: a long
        // paragraph is several labels, and their rows must be spaced like the rows inside one.
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
        let body = ui.style().text_styles.get(&TextStyle::Body).map(|f| f.size).unwrap_or(15.0);
        for it in items {
            match it {
                Inline::Text { text, strong, italic, strike, code, heading } => {
                    let mut rt = RichText::new(text).line_height(Some((body * 1.5).round()));
                    if let Some(h) = heading {
                        let size = match h {
                            1 => 24.0,
                            2 => 20.0,
                            3 => 17.0,
                            _ => 15.0,
                        };
                        rt = rt.font(FontId::new(size, crate::theme::bold())).strong().line_height(Some((size * 1.35_f32).round()));
                    }
                    if *code {
                        rt = rt.code();
                    }
                    if *strong && heading.is_none() && !*code {
                        rt = rt.family(crate::theme::bold()).strong();
                    }
                    if *italic {
                        rt = rt.italics();
                    }
                    if *strike {
                        rt = rt.strikethrough();
                    }
                    if wrap {
                        ui.label(rt);
                    } else {
                        ui.add(egui::Label::new(rt).extend());
                    }
                }
                Inline::Link { text, url } => {
                    ui.hyperlink_to(RichText::new(text).line_height(Some((body * 1.5).round())), url).on_hover_text(url);
                }
                Inline::Break => {
                    if wrap {
                        ui.end_row();
                    } else {
                        ui.label(" ");
                    }
                }
            }
        }
    }
}

fn code_block(ui: &mut Ui, lang: &str, code: &str) {
    let p = crate::theme::of(ui);
    egui::Frame::none()
        .fill(p.code_bg)
        .stroke(egui::Stroke::new(1.0_f32, p.border))
        .rounding(12.0)
        .inner_margin(egui::Margin { left: 12.0, right: 8.0, top: 6.0, bottom: 10.0 })
        .show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new(if lang.is_empty() { "code" } else { lang }).small().color(p.weak));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let id = ui.id().with("copied");
                let copied_at: Option<f64> = ui.data(|d| d.get_temp(id));
                let now = ui.input(|i| i.time);
                let recently = copied_at.map(|t| now - t < 1.5).unwrap_or(false);
                let label = if recently { "✔ Copied" } else { "📋 Copy" };
                let b = crate::theme::Btn::new(crate::theme::Kind::Ghost, RichText::new(label).small());
                if ui.add(b).on_hover_text("Copy this code to the clipboard").clicked() {
                    ui.ctx().copy_text(code.to_string());
                    ui.data_mut(|d| d.insert_temp(id, now));
                }
                if recently {
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(300));
                }
            });
        });
        egui::ScrollArea::horizontal().show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(code).text_style(TextStyle::Monospace)).extend());
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_markdown() {
        let md = "# Title\n\nSome **bold** and `code`.\n\n- one\n- two [link](https://x.y)\n\n```py\nprint(1)\n```\n\n| a | b |\n|---|--:|\n| 1 | 2 |\n\n> quote\n\n---\n";
        let b = parse(md);
        assert!(matches!(b[0], Block::Heading(1, _)));
        assert!(matches!(b[1], Block::Paragraph(_)));
        match &b[2] {
            Block::List { start: None, items } => {
                assert_eq!(items.len(), 2);
                let Block::Paragraph(inl) = &items[1][0] else { panic!() };
                assert!(inl.iter().any(|i| matches!(i, Inline::Link { url, .. } if url == "https://x.y")));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(b[3], Block::Code { lang: "py".into(), code: "print(1)".into() });
        match &b[4] {
            Block::Table { rows, align } => {
                assert_eq!(rows.len(), 2);
                assert_eq!(align[1], Alignment::Right);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(b[5], Block::Quote(_)));
        assert_eq!(b[6], Block::Rule);
    }

    #[test]
    fn brackets_do_not_split_text() {
        let b = parse("Mit zahlen[0] greifst du **zu** und x[1].");
        let Block::Paragraph(inl) = &b[0] else { panic!() };
        assert_eq!(inl.len(), 3, "{inl:?}");
        assert!(matches!(&inl[0], Inline::Text { text, .. } if text == "Mit zahlen[0] greifst du "));
    }

    #[test]
    fn unfinished_stream_does_not_panic() {
        for md in ["```py\nprint(", "| a |\n|--", "- [x", "**bo", "> > >"] {
            let _ = parse(md);
        }
    }
}
