use super::{Message, SourcesScreen};
use crate::appearance;
use crate::screen::keyboard_control::{button, focusable};
use iced::{
    Element, Fill,
    widget::{checkbox, column, container, image, progress_bar, row, scrollable, text, text_input},
};
use plume_source::ReleaseChange;
use rust_i18n::t;

impl SourcesScreen {
    pub fn view(&self) -> Element<'_, Message> {
        let add = row![
            text_input(&t!("sources_url_placeholder"), &self.input)
                .id("source-url")
                .on_input(Message::UrlChanged)
                .on_submit(Message::Add)
                .width(Fill),
            button(text(t!("sources_add")))
                .on_press_maybe((self.add.is_none() && self.storage_ok).then_some(Message::Add))
                .style(appearance::p_button)
        ]
        .spacing(8);
        let tabs = row![
            button(text(t!("sources_all")))
                .on_press(Message::SelectSource(None))
                .style(appearance::s_button),
            button(text(format!(
                "{} ({})",
                t!("sources_updates"),
                self.updates_count()
            )))
            .on_press(Message::ShowUpdates)
            .style(appearance::s_button),
            button(text(t!("sources_check")))
                .on_press(Message::RefreshAll)
                .style(appearance::s_button)
        ]
        .spacing(8);
        let mut content = column![tabs, add].spacing(10);
        if self.loading {
            content = content.push(text(t!("sources_loading")));
        }
        if self.add.is_some() {
            content = content.push(row![
                text(t!("sources_validating")),
                button(text(t!("cancel"))).on_press(Message::CancelAdd)
            ]);
        }
        if let Some(error) = &self.error {
            content = content.push(text(error).color(iced::Color::from_rgb(0.85, 0.25, 0.2)));
        }
        if let Some(error) = &self.device_error {
            content = content.push(text(format!("{}: {error}", t!("sources_device_error"))));
        }
        if let Some(download) = &self.download {
            let (received, total) = download.progress.snapshot();
            content = content.push(
                column![
                    text(format!(
                        "{}: {:.1} / {:.1} MiB",
                        t!("sources_downloading"),
                        received as f64 / 1048576.0,
                        total as f64 / 1048576.0
                    )),
                    progress_bar(
                        0.0..=100.0,
                        if total > 0 {
                            100.0 * received as f32 / total as f32
                        } else {
                            0.0
                        }
                    ),
                    button(text(t!("cancel"))).on_press(Message::CancelDownload)
                ]
                .spacing(5),
            );
        }
        let body = if self.detail.is_some() {
            self.view_detail()
        } else {
            self.view_catalogs()
        };
        content = content.push(body);
        container(content).height(Fill).width(Fill).into()
    }
    fn view_catalogs(&self) -> Element<'_, Message> {
        let mut content = column![].spacing(10);
        if self.store.sources.is_empty() {
            content = content.push(text(t!("sources_empty")));
        }
        for source in &self.store.sources {
            let name = format!(
                "{} · {} {}",
                source.catalog.name,
                source.catalog.apps.len(),
                t!("sources_apps")
            );
            let controls = row![
                button(text(name))
                    .on_press(Message::SelectSource(Some(source.key.clone())))
                    .style(appearance::s_button)
                    .width(Fill),
                focusable(
                    checkbox(source.enabled)
                        .label(t!("sources_enabled"))
                        .on_toggle({
                            let key = source.key.clone();
                            move |v| Message::Toggle(key.clone(), v)
                        }),
                    Some(Message::Toggle(source.key.clone(), !source.enabled))
                ),
                button(text(if self.requests.contains_key(&source.key) {
                    t!("sources_loading")
                } else {
                    t!("sources_refresh")
                }))
                .on_press_maybe(
                    (source.enabled && !self.requests.contains_key(&source.key))
                        .then_some(Message::Refresh(source.key.clone()))
                )
                .style(appearance::s_button),
                button(text(t!("sources_remove")))
                    .on_press(Message::Remove(source.key.clone()))
                    .style(appearance::s_button)
            ]
            .spacing(6);
            content = content
                .push(controls)
                .push(text(source.url.as_str()).size(11));
            if let Some(date) = source.last_success {
                content = content.push(
                    text(format!(
                        "{}: {}",
                        t!("sources_last_check"),
                        date.format("%Y-%m-%d %H:%M UTC")
                    ))
                    .size(11),
                );
            }
            if let Some(error) = &source.error {
                content = content.push(text(format!("{}: {error}", t!("sources_cached"))));
            }
            if self.selected_source.as_ref() == Some(&source.key) {
                if let Some(subtitle) = &source.catalog.subtitle {
                    content = content.push(text(subtitle));
                }
                if let Some(description) = &source.catalog.description {
                    content = content.push(
                        text(description)
                            .width(Fill)
                            .wrapping(text::Wrapping::WordOrGlyph),
                    );
                }
                for warning in &source.catalog.diagnostics {
                    content = content.push(text(warning).size(12));
                }
            }
        }
        content = content.push(
            text_input(&t!("sources_search"), &self.search)
                .id("source-search")
                .on_input(Message::Search),
        );
        if self.updates_only {
            content = content.push(text(t!("sources_updates_hint")));
        }
        let mut count = 0;
        for source in self.store.sources.iter().filter(|s| {
            (s.enabled || self.selected_source.as_ref() == Some(&s.key))
                && self
                    .selected_source
                    .as_ref()
                    .is_none_or(|key| key == &s.key)
        }) {
            for app in source.catalog.apps.iter().filter(|a| {
                a.matches(&self.search) && (!self.updates_only || self.update_available(source, a))
            }) {
                count += 1;
                if count > self.limit {
                    continue;
                }
                let latest = app
                    .newest_eligible(self.profile.as_ref())
                    .and_then(|i| app.releases.get(i));
                let title = format!(
                    "{}  {}",
                    app.name,
                    latest.map(|r| r.version.as_str()).unwrap_or("—")
                );
                let details = format!(
                    "{} · {}\n{}",
                    source.catalog.name,
                    app.developer.as_deref().unwrap_or("—"),
                    app.bundle_id
                );
                let status = if self.update_available(source, app) {
                    t!("sources_update").to_string()
                } else if self.record(source, app).is_some() {
                    t!("sources_tracked").to_string()
                } else {
                    String::new()
                };
                content = content.push(
                    button(
                        row![
                            container(text("▣").size(28)).width(40),
                            column![text(title), text(details).size(12), text(status).size(12)]
                                .spacing(3)
                        ]
                        .spacing(8),
                    )
                    .width(Fill)
                    .on_press(Message::Details(source.key.clone(), app.bundle_id.clone()))
                    .style(appearance::s_button),
                );
            }
        }
        if count == 0 {
            content = content.push(text(t!("sources_no_results")));
        }
        if count > self.limit {
            content = content.push(button(text(t!("sources_more"))).on_press(Message::More));
        }
        scrollable(content.width(Fill))
            .id("sources-body")
            .height(Fill)
            .width(Fill)
            .into()
    }
    fn view_detail(&self) -> Element<'_, Message> {
        let Some((source, app)) = self.detail_app() else {
            return text(t!("sources_no_results")).into();
        };
        let icon: Element<'_, Message> = app
            .icon_url
            .as_ref()
            .and_then(|u| self.icons.get(u.as_str()))
            .and_then(Option::as_ref)
            .map(|h| image(h.clone()).width(64).height(64).into())
            .unwrap_or_else(|| container(text("▣").size(40)).width(64).height(64).into());
        let mut content = column![
            button(text(t!("back")))
                .on_press(Message::Back)
                .style(appearance::s_button),
            row![
                icon,
                column![
                    text(&app.name).size(23),
                    text(format!(
                        "{} · {}",
                        source.catalog.name,
                        app.developer.as_deref().unwrap_or("—")
                    )),
                    text(&app.bundle_id).size(12)
                ]
                .spacing(5)
            ]
            .spacing(10)
        ]
        .spacing(10);
        if let Some(description) = &app.description {
            content = content.push(
                text(description)
                    .width(Fill)
                    .wrapping(text::Wrapping::WordOrGlyph),
            );
        }
        if let Some(reason) = &app.unavailable {
            content = content.push(text(reason));
        }
        if !source.enabled {
            content = content.push(text(t!("sources_disabled")));
        }
        if let Some(release) = app.releases.get(self.release) {
            content = content.push(text(format!(
                "{}{} · {} · {}",
                release.version,
                release
                    .build
                    .as_ref()
                    .map(|b| format!(" ({b})"))
                    .unwrap_or_default(),
                release.date.as_deref().unwrap_or("—"),
                release
                    .size
                    .map(|n| format!("{:.1} MiB", n as f64 / 1048576.0))
                    .unwrap_or_else(|| "—".into())
            )));
            content = content.push(text(format!(
                "OS {} – {} · {}",
                release.min_os.as_deref().unwrap_or("—"),
                release.max_os.as_deref().unwrap_or("—"),
                if release.platforms.is_empty() {
                    t!("sources_platform_pending").to_string()
                } else {
                    release.platforms.join(", ")
                }
            )));
            let incompatible = self
                .profile
                .as_ref()
                .and_then(|d| release.incompatibility(d));
            if let Some(reason) = &incompatible {
                content = content.push(text(reason.clone()));
            }
            let change = self.change(source, app, self.release);
            let label = match change {
                Some(ReleaseChange::Same) => t!("sources_installed"),
                Some(ReleaseChange::Newer) => t!("sources_update"),
                Some(ReleaseChange::Older) => t!("sources_install_older"),
                Some(ReleaseChange::Ambiguous) => t!("sources_install_changed"),
                None => t!("install"),
            };
            let label = if self.retry_download {
                format!("{}: {label}", t!("sources_retry"))
            } else {
                label.to_string()
            };
            if change == Some(ReleaseChange::Ambiguous) {
                content = content.push(text(t!("sources_ambiguous")));
            }
            let record = self.record(source, app);
            let absent = record.is_some_and(|r| {
                self.inventory
                    .as_ref()
                    .is_some_and(|ids| !ids.contains(&r.installed_bundle_id))
            });
            if absent {
                content = content.push(text(t!("sources_absent")));
            }
            if let Some(record) = record {
                content = content.push(
                    text(format!(
                        "{}: {}",
                        t!("sources_tracked"),
                        record.installed_bundle_id
                    ))
                    .size(12),
                );
            }
            let allowed = source.enabled
                && app.unavailable.is_none()
                && incompatible.is_none()
                && self.profile.is_some()
                && self.download.is_none()
                && change != Some(ReleaseChange::Same)
                && !absent;
            content = content.push(
                button(text(if absent {
                    t!("sources_unavailable").to_string()
                } else {
                    label
                }))
                .on_press_maybe(allowed.then_some(Message::Install))
                .style(appearance::p_button),
            );
            if self.profile.is_none() {
                content = content.push(text(t!("sources_select_device")));
            }
            if let Some(notes) = &release.notes {
                content = content.push(text(t!("sources_release_notes"))).push(
                    text(notes)
                        .width(Fill)
                        .wrapping(text::Wrapping::WordOrGlyph),
                );
            }
        }
        content = content.push(text(t!("sources_history")));
        for (index, release) in app.releases.iter().enumerate() {
            let reason = self
                .profile
                .as_ref()
                .and_then(|d| release.incompatibility(d));
            content = content.push(
                button(text(format!(
                    "{}{} · {}{}",
                    release.version,
                    release
                        .build
                        .as_ref()
                        .map(|b| format!(" ({b})"))
                        .unwrap_or_default(),
                    release.date.as_deref().unwrap_or("—"),
                    reason.map(|r| format!(" · {r}")).unwrap_or_default()
                )))
                .on_press(Message::SelectRelease(index))
                .style(appearance::s_button),
            );
        }
        scrollable(content.width(Fill))
            .id("sources-body")
            .height(Fill)
            .width(Fill)
            .into()
    }
}
