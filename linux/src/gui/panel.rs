// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The connection-info panel: `ConnectionPanel.java`, as a window over the
//! live desktop.
//!
//! Controls above facts, as on the phone: the panel's real job is "change
//! the quality", and the facts are what you read *after* deciding something
//! is wrong. The controls are the backend's live options from the registry,
//! rendered by their kind and nothing else, so the panel knows no option by
//! name; then the two that are not options — the desktop's size, where the
//! far end takes one, and which of its displays it is sending, where it
//! sends one at a time — which are facts about the connection in front of
//! you rather than descriptions of the backend; and whether the desktop
//! should follow this window's size.
//!
//! Then the desktop's name on top of what the backend has to say about the
//! connection in its own words, with the rows only worth reading when
//! hunting a problem folded under a heading, and the counts this client
//! keeps itself. The session refreshes the [`Snapshot`] once a second while
//! the panel is open, which is fast enough that a change reads as a result
//! and slow enough that the question costs the protocol nothing. Nothing is
//! rebuilt by a refresh: egui redraws from the same ids every frame, so an
//! open dropdown stays open and a field somebody is typing in keeps its
//! text until it is left.
//!
//! The panel does nothing to the backend itself. What is changed on it
//! comes back as [`Action`]s for the session to apply, which is what keeps
//! the panel a drawing of state rather than an owner of any.

use crate::backend::{ConnectionFact, Facts, Field, Monitor, Traffic};
use crate::options::{self, Kind, Options, Spec};
use egui::{Align2, CollapsingHeader, ComboBox, Grid, RichText, ScrollArea, TextEdit, Ui, vec2};
use std::collections::BTreeMap;

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

/// What the session's controls are set to, for drawing them.
pub struct Controls<'a> {
    /// The backend's options; only the live ones are drawn.
    pub specs: &'static [Spec],
    /// Every option's value right now: what the session connected with,
    /// then whatever has been set since. All of them rather than the live
    /// ones, since a gate may hang off one that is not.
    pub values: &'a Options,
    /// Whether the controls mean anything: none of them is live against a
    /// session that is connecting or gone.
    pub connected: bool,
    /// This window's size in desktop pixels, offered as a size.
    pub window: Option<(u32, u32)>,
    pub follow: bool,
}

/// Something changed on the panel, to be done to the session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    SetOption(&'static str, String),
    DesktopSize(u32, u32),
    Display(i32),
    Follow(bool),
}

/// The panel's own state between frames.
#[derive(Default)]
pub struct Panel {
    pub open: bool,
    /// What is being typed into a text field, per option, while it has the
    /// focus: a refresh must not rewrite a field under somebody's fingers,
    /// and the value is only sent once they leave it.
    edits: BTreeMap<&'static str, String>,
}

/// The sizes offered besides the window's and the desktop's own, which are
/// the RDP backends' list for the same reason they have one.
const SIZES: [(u32, u32); 6] = [
    (1920, 1200),
    (1920, 1080),
    (1600, 1000),
    (1366, 768),
    (1280, 800),
    (2560, 1440),
];

impl Panel {
    /// Draws the panel. `title` is what to call the session when the desktop
    /// has not named itself, and `state` the one-line status under it. What
    /// was changed on it this frame comes back as actions.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        snapshot: &Snapshot,
        controls: &Controls,
        title: &str,
        state: &str,
    ) -> Vec<Action> {
        let mut actions = Vec::new();
        let mut open = self.open;
        let room = (ctx.content_rect().height() - 48.0).max(120.0);
        egui::Window::new("Connection")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(360.0)
            // Drawn at its content's height, and scrolled once that is
            // taller than the window it floats over. The default height is
            // what the content is *offered* — an egui window offers its
            // content no more than that, and a scroll area never asks for
            // more — and not what is drawn, which follows the content: the
            // scroll area shrinks to it, where the window's own would not.
            .default_height(room)
            .max_height(room)
            .anchor(Align2::RIGHT_TOP, vec2(-16.0, 16.0))
            .show(ctx, |ui| {
                ui.heading(snapshot.desktop_name().unwrap_or(title));
                ui.label(RichText::new(state).weak());
                ui.add_space(8.0);
                ScrollArea::vertical().show(ui, |ui| {
                    ui.add_enabled_ui(controls.connected, |ui| {
                        self.controls(ui, snapshot, controls, &mut actions);
                    });
                    facts(ui, snapshot);
                });
            });
        self.open = open;
        actions
    }

    fn controls(&mut self, ui: &mut Ui, snapshot: &Snapshot, controls: &Controls, actions: &mut Vec<Action>) {
        let live: Vec<&'static Spec> = controls.specs.iter().filter(|s| s.live).collect();
        let facts = &snapshot.facts;
        let values = controls.values;
        Grid::new("controls").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
            for spec in live {
                // The same two rules the dialog draws by: an option whose
                // gate is shut is not there, and a choice whose switch is
                // off is not offered.
                if let Some(gate) = spec.gate
                    && !gate.open(values.get(gate.key))
                {
                    continue;
                }
                self.option_row(ui, spec, values, actions);
            }

            if facts.can_resize {
                let have = (facts.width as u32, facts.height as u32);
                let label = ui.label(RichText::new("Desktop size").weak());
                label.on_hover_text("Ask the far end to make its desktop this size; what arrives is what it granted.");
                ComboBox::from_id_salt("desktop-size")
                    .selected_text(size_label(have))
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for (size, label) in size_choices(have, controls.window, &facts.monitors) {
                            if ui.selectable_label(size == have, label).clicked() && size != have {
                                actions.push(Action::DesktopSize(size.0, size.1));
                            }
                        }
                    });
                ui.end_row();
            }

            if !facts.displays.is_empty() {
                let label = ui.label(RichText::new("Display").weak());
                label.on_hover_text("Which of the far end's screens it is sending; a switch is a request.");
                let shown = facts
                    .displays
                    .get(facts.display.max(0) as usize)
                    .map(|m| display_label(facts.display.max(0), m))
                    .unwrap_or_else(|| format!("{} available", facts.displays.len()));
                ComboBox::from_id_salt("display")
                    .selected_text(shown)
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for (i, m) in facts.displays.iter().enumerate() {
                            let i = i as i32;
                            if ui.selectable_label(i == facts.display, display_label(i, m)).clicked()
                                && i != facts.display
                            {
                                actions.push(Action::Display(i));
                            }
                        }
                    });
                ui.end_row();
            }

            let label = ui.label(RichText::new("Follow window size").weak());
            label.on_hover_text(
                "Ask the far end for a desktop the size of this window, in pixels, whenever the window \
                 changes shape — once it has settled, and once the far end says it takes sizes.",
            );
            let mut follow = controls.follow;
            let note = if follow && !facts.can_resize {
                "waiting for the far end to take a size"
            } else {
                ""
            };
            if ui.checkbox(&mut follow, RichText::new(note).weak().italics()).changed() {
                actions.push(Action::Follow(follow));
            }
            ui.end_row();
        });
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);
    }

    /// One live option as the control its kind calls for, showing what it
    /// is set to now.
    fn option_row(&mut self, ui: &mut Ui, spec: &'static Spec, values: &Options, actions: &mut Vec<Action>) {
        let label = ui.label(RichText::new(spec.label).weak());
        if let Some(summary) = spec.summary {
            label.on_hover_text(summary);
        }
        let current = values.get(spec.key);
        match spec.kind {
            Kind::Bool => {
                let mut on = current.eq_ignore_ascii_case("true");
                if ui.checkbox(&mut on, "").changed() {
                    actions.push(Action::SetOption(spec.key, if on { "true" } else { "false" }.to_string()));
                }
            }
            Kind::Choice => {
                let offered: Vec<&options::Choice> = spec
                    .choices
                    .iter()
                    .filter(|c| c.requires.is_none_or(|key| values.bool(key)))
                    .collect();
                ComboBox::from_id_salt(("live", spec.key))
                    .selected_text(spec.label_for(current).to_string())
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for choice in offered {
                            let mut row = ui.selectable_label(choice.value == current, choice.label);
                            if let Some(summary) = choice.summary {
                                row = row.on_hover_text(summary);
                            }
                            if row.clicked() && choice.value != current {
                                actions.push(Action::SetOption(spec.key, choice.value.to_string()));
                            }
                        }
                    });
            }
            Kind::Int | Kind::Text => {
                let buffer = self.edits.entry(spec.key).or_insert_with(|| current.to_string());
                let field = ui.add(TextEdit::singleline(buffer).desired_width(f32::INFINITY));
                if field.lost_focus() {
                    let typed = self.edits.remove(spec.key).unwrap_or_default();
                    if typed != current {
                        match spec.check(&typed) {
                            Ok(()) => actions.push(Action::SetOption(spec.key, typed)),
                            Err(why) => log::warn!("{why}; left as it was"),
                        }
                    }
                } else if !field.has_focus() {
                    // Refreshed only while nobody is typing in it.
                    *buffer = current.to_string();
                }
            }
        }
        ui.end_row();
    }
}

/// What to offer: this window first, since fitting it is the whole point;
/// then the size the desktop already has, so the current state has a row of
/// its own; the far end's own screens, where it has said how it is divided;
/// then the ordinary ones. No backend here lists the sizes its far end would
/// take — RFB, RDP and SPICE all take any — so there is nothing to ask.
fn size_choices(have: (u32, u32), window: Option<(u32, u32)>, monitors: &[Monitor]) -> Vec<((u32, u32), String)> {
    let mut choices: Vec<((u32, u32), String)> = Vec::new();
    let mut offer = |size: (u32, u32), label: String| {
        if size.0 > 0 && size.1 > 0 && !choices.iter().any(|(s, _)| *s == size) {
            choices.push((size, label));
        }
    };
    if let Some(window) = window {
        offer(window, format!("This window · {}", size_label(window)));
    }
    offer(have, size_label(have));
    for m in monitors {
        let size = (m.width.max(0) as u32, m.height.max(0) as u32);
        offer(size, size_label(size));
    }
    for size in SIZES {
        offer(size, size_label(size));
    }
    choices
}

fn size_label((w, h): (u32, u32)) -> String {
    format!("{w} × {h}")
}

fn display_label(i: i32, m: &Monitor) -> String {
    format!("{} · {} × {}", i + 1, m.width, m.height)
}

/// The facts: the backend's rows, then the counts this client keeps.
fn facts(ui: &mut Ui, snapshot: &Snapshot) {
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
        // The count, unless the backend already put it among its own rows
        // (the Java twins of most of them do).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_comes_first_and_nothing_is_offered_twice() {
        let monitors = [
            Monitor {
                x: 0,
                y: 0,
                width: 1920,
                height: 1200,
            },
            Monitor {
                x: 1920,
                y: 0,
                width: 1280,
                height: 800,
            },
        ];
        let choices = size_choices((1920, 1200), Some((1600, 900)), &monitors);
        let sizes: Vec<(u32, u32)> = choices.iter().map(|(s, _)| *s).collect();
        assert_eq!(sizes[0], (1600, 900));
        assert!(choices[0].1.starts_with("This window"));
        assert_eq!(sizes[1], (1920, 1200));
        assert_eq!(sizes[2], (1280, 800));
        assert_eq!(sizes.iter().filter(|s| **s == (1920, 1200)).count(), 1);
        assert_eq!(sizes.iter().filter(|s| **s == (1280, 800)).count(), 1);
        // The window, the desktop, the second monitor, and the four of the
        // usual list that are not one of those.
        assert_eq!(sizes.len(), 7);
    }

    #[test]
    fn a_desktop_with_no_size_yet_is_not_a_choice() {
        let choices = size_choices((0, 0), None, &[]);
        assert_eq!(choices.len(), SIZES.len());
    }
}
