// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The connection-info panel: `ConnectionPanel.java` without the controls,
//! as a window over the live desktop.
//!
//! The desktop's name on top, then what the backend has to say about the
//! connection in its own words, with the rows only worth reading when
//! hunting a problem folded under a heading; then the counts this client
//! keeps itself — what was transferred, how big the desktop is, how it is
//! divided. The session refreshes the [`Snapshot`] once a second while the
//! panel is open, which is fast enough that a change reads as a result and
//! slow enough that the question costs the protocol nothing.

use crate::backend::{ConnectionFact, Facts, Field, Traffic};
use egui::{Align2, CollapsingHeader, Grid, RichText, Ui, vec2};

/// What the backend said at the last poll: one instant, so that no row is
/// drawn from one answer and its neighbour from another.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub facts: Facts,
    pub info: Vec<ConnectionFact>,
    pub traffic: Option<Traffic>,
}

impl Snapshot {
    /// The desktop's own name for itself, where the protocol has said it.
    pub fn desktop_name(&self) -> Option<&str> {
        self.info
            .iter()
            .find(|f| f.field == Field::DesktopName)
            .map(|f| f.value.as_str())
            .filter(|v| !v.is_empty())
    }
}

/// Draws the panel. `title` is what to call the session when the desktop
/// has not named itself, and `state` the one-line status under it.
pub fn show(ctx: &egui::Context, open: &mut bool, snapshot: &Snapshot, title: &str, state: &str) {
    egui::Window::new("Connection")
        .open(open)
        .collapsible(false)
        .resizable(false)
        .default_width(360.0)
        .anchor(Align2::RIGHT_TOP, vec2(-16.0, 16.0))
        .show(ctx, |ui| {
            ui.heading(snapshot.desktop_name().unwrap_or(title));
            ui.label(RichText::new(state).weak());
            ui.add_space(8.0);

            let plain: Vec<&ConnectionFact> = snapshot
                .info
                .iter()
                .filter(|f| f.field != Field::DesktopName && !f.diagnostic)
                .collect();
            let diagnostics: Vec<&ConnectionFact> = snapshot
                .info
                .iter()
                .filter(|f| f.field != Field::DesktopName && f.diagnostic)
                .collect();

            Grid::new("facts").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                for fact in &plain {
                    row(ui, fact.label, &fact.value);
                }
                // The count, unless the backend already put it among its
                // own rows (the Java twins of most of them do).
                if let Some(traffic) = snapshot.traffic
                    && !plain.iter().any(|f| f.field == Field::Data)
                {
                    let data = ConnectionFact::data(traffic);
                    row(ui, data.label, &data.value);
                }
                let facts = &snapshot.facts;
                if facts.width > 0 && facts.height > 0 {
                    row(ui, "Desktop size", &format!("{} × {}", facts.width, facts.height));
                }
                for (i, m) in facts.monitors.iter().enumerate() {
                    row(
                        ui,
                        if i == 0 { "Monitors" } else { "" },
                        &format!("{} · {} × {} at {}, {}", i + 1, m.width, m.height, m.x, m.y),
                    );
                }
                if facts.displays.len() > 1 {
                    let shown = facts
                        .displays
                        .get(facts.display.max(0) as usize)
                        .map(|m| format!("{} of {} · {} × {}", facts.display + 1, facts.displays.len(), m.width, m.height))
                        .unwrap_or_else(|| format!("{} available", facts.displays.len()));
                    row(ui, "Display", &shown);
                }
            });

            if !diagnostics.is_empty() {
                ui.add_space(8.0);
                CollapsingHeader::new("Diagnostics").default_open(false).show(ui, |ui| {
                    Grid::new("diagnostics").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                        for fact in &diagnostics {
                            row(ui, fact.label, &fact.value);
                        }
                    });
                });
            }
        });
}

/// A field the backend has but no answer for yet says so, rather than
/// vanishing: the shape of the panel depends on the protocol, not on how
/// much this server has got round to saying.
fn row(ui: &mut Ui, label: &str, value: &str) {
    ui.label(RichText::new(label).weak());
    if value.is_empty() {
        ui.label(RichText::new("Not set").weak().italics());
    } else {
        ui.label(value);
    }
    ui.end_row();
}
