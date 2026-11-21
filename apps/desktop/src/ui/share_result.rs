use super::{Capsule, INK, MUTED, SURFACE, button};
use crate::{
    backend::Event, model::Visibility, received_files, scroll_area::ScrollbarExt,
    shared_text::ReceivedText,
};
use gpui::prelude::FluentBuilder;
use gpui::*;
use std::{
    path::PathBuf,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use wayline_host::TransferHistoryRecord;

pub(super) struct ShareResult {
    id: Instant,
    pub(super) title: String,
    pub(super) message: String,
    text: Option<ReceivedText>,
    pub(super) image: Option<Arc<RenderImage>>,
    files: Vec<PathBuf>,
    history: Option<TransferHistoryRecord>,
    copied: bool,
    answered: bool,
    busy: bool,
    hovered: bool,
    feedback: Option<(String, bool)>,
    deadline: Instant,
    pub(super) previous_visibility: Visibility,
}

#[derive(Clone, Copy)]
enum FileAction {
    Open,
    Save,
    Move,
}

#[derive(Clone)]
struct ShareFileWork {
    id: Instant,
    action: FileAction,
    files: Vec<PathBuf>,
    history: Option<TransferHistoryRecord>,
}

pub(crate) struct ShareFileDestination {
    work: ShareFileWork,
    directory: Result<Option<PathBuf>, String>,
}

pub(crate) struct ShareFileActionResult {
    id: Instant,
    history: Option<TransferHistoryRecord>,
    result: Result<Option<received_files::MoveOutcome>, String>,
    performed: bool,
}

impl ShareResult {
    pub(super) fn new(
        title: String,
        message: String,
        text: Option<ReceivedText>,
        image: Option<Arc<RenderImage>>,
        files: Vec<PathBuf>,
        history: Option<TransferHistoryRecord>,
    ) -> Self {
        let id = Instant::now();
        Self {
            id,
            title,
            message,
            text,
            image,
            files,
            history,
            copied: false,
            answered: false,
            busy: false,
            hovered: false,
            feedback: None,
            deadline: id,
            previous_visibility: Visibility::Hidden,
        }
    }

    fn needs_decision(&self) -> bool {
        !self.answered
            && !self.files.is_empty()
            && self
                .text
                .as_ref()
                .is_some_and(|text| text.character_count > 200)
    }

    pub(super) fn can_dismiss(&self, now: Instant) -> bool {
        !self.busy && !self.hovered && !self.needs_decision() && now >= self.deadline
    }

    pub(super) fn renew_deadline(&mut self, seconds: u64) {
        self.deadline = Instant::now() + Duration::from_secs(seconds);
    }
}

impl Capsule {
    fn copy_share_result(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(result) = self.share_result.as_mut().filter(|result| !result.busy) else {
            return;
        };
        if let Some(text) = &result.text {
            cx.write_to_clipboard(ClipboardItem::new_string(text.original.clone()));
            result.copied = true;
            result.answered = true;
        } else {
            match crate::native::hwnd(window)
                .and_then(|owner| received_files::copy_to_clipboard(&result.files, owner))
            {
                Ok(()) => {
                    result.copied = true;
                    result.answered = true;
                }
                Err(error) => result.feedback = Some((error.to_string(), true)),
            }
        }
        result.renew_deadline(self.completion_duration_seconds);
        cx.notify();
    }

    fn run_share_file_action(
        &mut self,
        action: FileAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(result) = self.share_result.as_mut().filter(|result| !result.busy) else {
            return;
        };
        let work = ShareFileWork {
            id: result.id,
            action,
            files: result.files.clone(),
            history: result.history.clone(),
        };
        result.busy = true;
        result.feedback = None;
        let label = match action {
            FileAction::Open => {
                self.spawn_share_file_work(work, None, cx);
                return;
            }
            FileAction::Save => self.language.strings().save,
            FileAction::Move => self.language.strings().share_move,
        };
        let fallback = work.clone();
        let tx = self.tx.clone();
        let picker = crate::native::hwnd(window).and_then(|owner| {
            let owner = owner.0 as usize;
            thread::Builder::new()
                .name("wayline-transfer-destination".into())
                .spawn(move || {
                    let directory = crate::picker::choose_transfer_destination(owner, label)
                        .map_err(|error| format!("{error:#}"));
                    let _ = tx.send(Event::ShareFileDestination(ShareFileDestination {
                        work,
                        directory,
                    }));
                })
                .map_err(Into::into)
        });
        if let Err(error) = picker {
            self.apply_share_file_destination(
                ShareFileDestination {
                    work: fallback,
                    directory: Err(error.to_string()),
                },
                cx,
            );
        }
        cx.notify();
    }

    pub(super) fn apply_share_file_destination(
        &mut self,
        selection: ShareFileDestination,
        cx: &mut Context<Self>,
    ) {
        match selection.directory {
            Ok(Some(directory)) => self.spawn_share_file_work(selection.work, Some(directory), cx),
            other => self.finish_share_file_action(
                ShareFileActionResult {
                    id: selection.work.id,
                    history: selection.work.history,
                    result: other.map(|_| None),
                    performed: false,
                },
                cx,
            ),
        }
    }

    fn spawn_share_file_work(
        &mut self,
        work: ShareFileWork,
        directory: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let id = work.id;
        let tx = self.tx.clone();
        match thread::Builder::new()
            .name("wayline-received-file-action".into())
            .spawn(move || {
                let result = match (work.action, directory) {
                    (FileAction::Open, _) => received_files::open_files(&work.files).map(|_| None),
                    (FileAction::Save, Some(directory)) => {
                        received_files::save_as(&work.files, &directory).map(|_| None)
                    }
                    (FileAction::Move, Some(directory)) => {
                        received_files::move_to(&work.files, &directory).map(Some)
                    }
                    _ => unreachable!("file relocation requires a selected directory"),
                }
                .map_err(|error| format!("{error:#}"));
                let _ = tx.send(Event::ShareFileAction(ShareFileActionResult {
                    id,
                    history: work.history,
                    result,
                    performed: true,
                }));
            }) {
            Ok(worker) => self.workers.push(worker),
            Err(error) => self.finish_share_file_action(
                ShareFileActionResult {
                    id,
                    history: None,
                    result: Err(error.to_string()),
                    performed: false,
                },
                cx,
            ),
        }
        cx.notify();
    }

    pub(super) fn finish_share_file_action(
        &mut self,
        outcome: ShareFileActionResult,
        cx: &mut Context<Self>,
    ) {
        if let (Ok(Some(moved)), Some(record), Some(worker)) =
            (&outcome.result, &outcome.history, &self.history_worker)
        {
            worker.relocate_received(record.clone(), moved.paths.clone());
        }
        let Some(result) = self
            .share_result
            .as_mut()
            .filter(|result| result.id == outcome.id)
        else {
            return;
        };
        let labels = self.language.strings();
        result.busy = false;
        result.renew_deadline(self.completion_duration_seconds);
        match outcome.result {
            Ok(Some(moved)) => {
                if let Some(record) = &mut result.history {
                    record.file_paths.clone_from(&moved.paths);
                }
                result.files = moved.paths;
                result.answered |= moved.warnings.is_empty();
                result.feedback = Some(if moved.warnings.is_empty() {
                    (labels.share_action_done.into(), false)
                } else {
                    (moved.warnings.join("\n"), true)
                });
            }
            Ok(None) if outcome.performed => {
                result.answered = true;
                result.feedback = Some((labels.share_action_done.into(), false));
            }
            Ok(None) => {}
            Err(message) => result.feedback = Some((message, true)),
        }
        cx.notify();
    }

    pub(super) fn share_result_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(result) = self.share_result.as_ref() else {
            return div().into_any_element();
        };
        let labels = self.language.strings();
        let preview = result.text.as_ref().map(|text| text.preview.as_str());
        let light = self.send_layout == super::compact_send::SendLayout::NekoLight;
        let ink = if light { 0x172538 } else { INK };
        let muted = if light { 0x495a70 } else { MUTED };
        let surface = if light { 0xeef2f7 } else { SURFACE };

        let mut content = div()
            .id("share-result-content")
            .w_full()
            .min_h_0()
            .flex_1()
            .visible_scrollbar()
            .flex()
            .flex_col()
            .gap(px(9.));
        if !result.message.is_empty() && preview != Some(result.message.as_str()) {
            content = content.child(
                div()
                    .flex_shrink_0()
                    .whitespace_normal()
                    .text_size(px(12.))
                    .line_height(px(17.))
                    .text_color(rgb(muted))
                    .child(result.message.clone()),
            );
        }
        if let Some(image) = &result.image {
            let height = if preview.is_some() { 96. } else { 142. };
            content = content.child(
                div()
                    .w_full()
                    .h(px(height))
                    .flex_shrink_0()
                    .rounded(px(9.))
                    .overflow_hidden()
                    .bg(rgb(if light { 0xe3e9f2 } else { 0x19191d }))
                    .child(
                        img(Arc::clone(image))
                            .size_full()
                            .object_fit(ObjectFit::Contain),
                    ),
            );
        }
        if let Some(text) = preview {
            content = content.child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex_shrink_0()
                    .whitespace_normal()
                    .text_color(rgb(ink))
                    .text_size(px(13.))
                    .line_height(px(19.))
                    .child(text.to_owned()),
            );
        }
        let mut actions = div().flex_shrink_0().flex().flex_wrap().gap(px(5.));
        let has_files = !result.files.is_empty();
        if has_files {
            for (id, label, action) in [
                ("share-result-open", labels.share_open, FileAction::Open),
                ("share-result-save", labels.save, FileAction::Save),
            ] {
                actions = actions.child(result_button(id, label, false, result.busy).on_click(
                    cx.listener(move |this, _, window, cx| {
                        this.run_share_file_action(action, window, cx);
                    }),
                ));
            }
        }
        if has_files || preview.is_some() {
            let label = if result.copied {
                labels.copied
            } else {
                labels.copy
            };
            actions = actions.child(
                result_button("share-result-copy", label, false, result.busy).on_click(
                    cx.listener(|this, _, window, cx| {
                        this.copy_share_result(window, cx);
                    }),
                ),
            );
        }
        if has_files {
            actions = actions.child(
                result_button("share-result-move", labels.share_move, false, result.busy).on_click(
                    cx.listener(|this, _, window, cx| {
                        this.run_share_file_action(FileAction::Move, window, cx);
                    }),
                ),
            );
        }
        let dismiss = if result.needs_decision() {
            labels.cancel
        } else {
            labels.done
        };
        actions = actions.child(
            result_button("share-result-done", dismiss, true, result.busy).on_click(cx.listener(
                |this, _, window, cx| {
                    if this
                        .share_result
                        .as_ref()
                        .is_some_and(|result| !result.busy)
                    {
                        this.finish_share_result(window, cx);
                    }
                },
            )),
        );
        div()
            .id("share-result-card")
            .w_full()
            .min_h_0()
            .flex_1()
            .flex()
            .flex_col()
            .gap(px(10.))
            .rounded(px(14.))
            .p(px(14.))
            .bg(rgb(surface))
            .on_hover(cx.listener(|this, hovering, _, cx| {
                if let Some(result) = &mut this.share_result {
                    result.hovered = *hovering;
                    result.renew_deadline(this.completion_duration_seconds);
                    cx.notify();
                }
            }))
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(rgb(ink))
                    .text_size(px(16.))
                    .line_height(px(21.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(result.title.clone()),
            )
            .child(content)
            .children(result.feedback.as_ref().map(|(message, error)| {
                div()
                    .flex_shrink_0()
                    .truncate()
                    .text_size(px(11.))
                    .text_color(rgb(if *error {
                        if light { 0x88451c } else { 0xffb56b }
                    } else {
                        muted
                    }))
                    .child(message.clone())
            }))
            .child(actions)
            .into_any_element()
    }
}

fn result_button(
    id: &'static str,
    label: &'static str,
    primary: bool,
    busy: bool,
) -> Stateful<Div> {
    button(id, label, primary)
        .h(px(28.))
        .px(px(8.))
        .rounded(px(7.))
        .text_size(px(11.))
        .when(busy, |button| button.opacity(0.55))
}
