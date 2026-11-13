use std::time::Duration;

use ratatui::{
    buffer::Buffer,
    layout::{Constraint::Length, Layout, Rect},
    widgets::Widget,
};

use super::{
    failures_with_own_cause,
    screen_chrome::{Footer, Header},
    screen_spacing::{RULED_ROW_HEIGHT, TEXT_ROW_HEIGHT},
    stage_summary::Stages,
};
use crate::{
    boot_model::{BootState, boot_origin::BootOrigin},
    screen_settings::{color_scheme::Colors, font_glyphs::Glyphs},
};

pub struct ShellHandoverSummary<'a> {
    pub state: &'a BootState,
    pub colors: &'a Colors,
    pub glyphs: &'a Glyphs,
    pub now: Duration,
    pub origin: &'a BootOrigin,
}

impl ShellHandoverSummary<'_> {
    fn stages(&self) -> Stages<'_> {
        Stages {
            state: self.state,
            now: self.now,
            colors: &self.colors.stages,
            glyphs: self.glyphs,
        }
    }

    #[must_use]
    pub fn height(&self) -> u16 {
        TEXT_ROW_HEIGHT
            + self.stages().height()
            + failures_with_own_cause(self.state, self.colors, self.glyphs).height()
            + RULED_ROW_HEIGHT
    }
}

impl Widget for ShellHandoverSummary<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let stages = self.stages();
        let failure_panel = failures_with_own_cause(self.state, self.colors, self.glyphs);
        let failure_height = failure_panel.height();
        let [header_area, stage_area, failure_area, footer_area] =
            area.layout(&Layout::vertical([
                Length(TEXT_ROW_HEIGHT),
                Length(stages.height()),
                Length(failure_height),
                Length(RULED_ROW_HEIGHT),
            ]));
        Header {
            origin: self.origin,
            total: Some(self.now.saturating_sub(self.state.origin)),
            colors: &self.colors.header,
        }
        .render(header_area, buf);
        stages.render(stage_area, buf);
        if failure_height > 0 {
            failure_panel.render(failure_area, buf);
        }
        Footer {
            tally: self.state.tally(),
            colors: &self.colors.footer,
            glyphs: self.glyphs,
        }
        .render(footer_area, buf);
    }
}
