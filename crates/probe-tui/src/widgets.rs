use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph, Tabs, Wrap};

fn shell_border() -> Style {
    Style::default().fg(Color::Rgb(0x73, 0xc2, 0xfb))
}

#[allow(dead_code)]
fn shell_accent() -> Style {
    Style::default()
        .fg(Color::Rgb(0xf8, 0xf4, 0xe3))
        .bg(Color::Rgb(0x13, 0x26, 0x3a))
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn padded_title(title: &str) -> String {
    format!("─ {title} ")
}

fn panel_padding() -> Padding {
    Padding::horizontal(1)
}

#[allow(dead_code)]
pub struct TabStrip {
    labels: Vec<String>,
    selected: usize,
}

#[allow(dead_code)]
impl TabStrip {
    pub fn new(labels: Vec<String>, selected: usize) -> Self {
        Self { labels, selected }
    }

    pub fn render(self, frame: &mut Frame<'_>, area: Rect) {
        let max_index = self.labels.len().saturating_sub(1);
        let tabs = Tabs::new(self.labels)
            .select(self.selected.min(max_index))
            .padding(" ", " ")
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .padding(panel_padding())
                    .style(shell_border()),
            )
            .highlight_style(shell_accent());
        frame.render_widget(tabs, area);
    }
}

pub struct InfoPanel<'a> {
    title: &'a str,
    body: Text<'a>,
    scroll_y: u16,
}

impl<'a> InfoPanel<'a> {
    pub const fn new(title: &'a str, body: Text<'a>) -> Self {
        Self {
            title,
            body,
            scroll_y: 0,
        }
    }

    pub const fn with_scroll(mut self, scroll_y: u16) -> Self {
        self.scroll_y = scroll_y;
        self
    }

    pub fn render(self, frame: &mut Frame<'_>, area: Rect) {
        frame.render_widget(
            Paragraph::new(self.body)
                .scroll((self.scroll_y, 0))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .padding(panel_padding())
                        .title(padded_title(self.title))
                        .style(shell_border()),
                )
                .wrap(Wrap { trim: false }),
            area,
        );
    }
}

pub struct ModalCard<'a> {
    title: &'a str,
    body: Paragraph<'a>,
}

impl<'a> ModalCard<'a> {
    pub const fn new(title: &'a str, body: Paragraph<'a>) -> Self {
        Self { title, body }
    }

    pub fn render(self, frame: &mut Frame<'_>, area: Rect) {
        frame.render_widget(Clear, area);
        frame.render_widget(
            Block::default().style(Style::default().bg(Color::Rgb(0x10, 0x17, 0x20))),
            area,
        );
        let vertical = Layout::vertical([Constraint::Percentage(65)])
            .flex(Flex::Center)
            .split(area);
        let horizontal = Layout::horizontal([Constraint::Percentage(72)])
            .flex(Flex::Center)
            .split(vertical[0]);
        let modal_area = horizontal[0];
        let block = Block::default()
            .borders(Borders::ALL)
            .padding(panel_padding())
            .title(padded_title(self.title))
            .style(
                Style::default()
                    .fg(Color::Rgb(0xff, 0xf1, 0xd0))
                    .bg(Color::Rgb(0x17, 0x24, 0x2f)),
            );
        let inner = block.inner(modal_area);

        frame.render_widget(Clear, modal_area);
        frame.render_widget(block, modal_area);
        frame.render_widget(
            self.body
                .alignment(Alignment::Left)
                .wrap(Wrap { trim: false }),
            inner,
        );
    }
}
