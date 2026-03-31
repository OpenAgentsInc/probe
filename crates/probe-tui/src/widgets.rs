use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Tabs, Wrap};

use crate::screens::ActiveTab;

fn shell_border() -> Style {
    Style::default().fg(Color::Rgb(0x73, 0xc2, 0xfb))
}

fn shell_accent() -> Style {
    Style::default()
        .fg(Color::Rgb(0xf8, 0xf4, 0xe3))
        .bg(Color::Rgb(0x13, 0x26, 0x3a))
        .add_modifier(Modifier::BOLD)
}

pub struct HeaderBar<'a> {
    title: &'a str,
    subtitle: &'a str,
    focus: &'a str,
}

impl<'a> HeaderBar<'a> {
    pub const fn new(title: &'a str, subtitle: &'a str, focus: &'a str) -> Self {
        Self {
            title,
            subtitle,
            focus,
        }
    }

    pub fn render(self, frame: &mut Frame<'_>, area: Rect) {
        let block = Block::default().borders(Borders::ALL).style(shell_border());
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let lines = vec![
            Line::styled(self.title, shell_accent()),
            Line::from(format!("{} | focus={}", self.subtitle, self.focus)),
        ];
        frame.render_widget(Paragraph::new(lines).alignment(Alignment::Left), inner);
    }
}

pub struct FooterBar<'a> {
    status: &'a str,
}

impl<'a> FooterBar<'a> {
    pub const fn new(status: &'a str) -> Self {
        Self { status }
    }

    pub fn render(self, frame: &mut Frame<'_>, area: Rect) {
        let block = Block::default().borders(Borders::ALL).style(shell_border());
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let lines = vec![
            Line::from("Tab/Arrow switch view | t toggle body | ? help | q quit"),
            Line::styled(
                self.status,
                Style::default().fg(Color::Rgb(0xf1, 0xc4, 0x53)),
            ),
        ];
        frame.render_widget(Paragraph::new(lines), inner);
    }
}

pub struct TabStrip<'a> {
    title: &'a str,
    subtitle: &'a str,
    active_tab: ActiveTab,
}

impl<'a> TabStrip<'a> {
    pub const fn new(title: &'a str, subtitle: &'a str, active_tab: ActiveTab) -> Self {
        Self {
            title,
            subtitle,
            active_tab,
        }
    }

    pub fn render(self, frame: &mut Frame<'_>, area: Rect) {
        let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(2)]).split(area);
        frame.render_widget(
            Paragraph::new(self.subtitle).style(Style::default().fg(Color::Rgb(0xa7, 0xc9, 0xe8))),
            rows[0],
        );
        let tabs = Tabs::new(vec!["Overview", "Events"])
            .select(match self.active_tab {
                ActiveTab::Overview => 0,
                ActiveTab::Events => 1,
            })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(self.title)
                    .style(shell_border()),
            )
            .highlight_style(shell_accent());
        frame.render_widget(tabs, rows[1]);
    }
}

pub struct InfoPanel<'a> {
    title: &'a str,
    body: Text<'a>,
}

impl<'a> InfoPanel<'a> {
    pub const fn new(title: &'a str, body: Text<'a>) -> Self {
        Self { title, body }
    }

    pub fn render(self, frame: &mut Frame<'_>, area: Rect) {
        frame.render_widget(
            Paragraph::new(self.body)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(self.title)
                        .style(shell_border()),
                )
                .wrap(Wrap { trim: false }),
            area,
        );
    }
}

pub struct SidebarPanel {
    title: &'static str,
    lines: Vec<String>,
}

impl SidebarPanel {
    pub fn new(title: &'static str, lines: Vec<String>) -> Self {
        Self { title, lines }
    }

    pub fn render(self, frame: &mut Frame<'_>, area: Rect) {
        let items = self
            .lines
            .into_iter()
            .map(ListItem::new)
            .collect::<Vec<_>>();
        frame.render_widget(
            List::new(items).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(self.title)
                    .style(shell_border()),
            ),
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
            .title(self.title)
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
