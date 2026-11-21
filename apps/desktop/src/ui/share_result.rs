use super::{Capsule, INK, MUTED, SURFACE, button};
use crate::{model::Visibility, scroll_area::ScrollbarExt};
use gpui::*;
use std::{sync::Arc, time::Instant};

pub(super) struct ShareResult {
    pub(super) title: String,
    pub(super) message: String,
    pub(super) original: Option<String>,
    pub(super) image: Option<Arc<RenderImage>>,
    pub(super) copied: bool,
    pub(super) deadline: Instant,
    pub(super) previous_visibility: Visibility,
}

impl Capsule {
    pub(super) fn share_result_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(result) = self.share_result.as_ref() else {
            return div().into_any_element();
        };
        let labels = self.language.strings();
        let original = result.original.as_ref().filter(|text| !text.is_empty());

        let mut content = div()
            .id("share-result-content")
            .w_full()
            .min_h_0()
            .flex_1()
            .visible_scrollbar()
            .flex()
            .flex_col()
            .gap(px(9.));
        if !result.message.is_empty() && original != Some(&result.message) {
            content = content.child(
                div()
                    .flex_shrink_0()
                    .whitespace_normal()
                    .text_size(px(12.))
                    .line_height(px(17.))
                    .text_color(rgb(MUTED))
                    .child(result.message.clone()),
            );
        }
        if let Some(image) = &result.image {
            let height = if original.is_some() { 96. } else { 142. };
            content = content.child(
                div()
                    .w_full()
                    .h(px(height))
                    .flex_shrink_0()
                    .rounded(px(9.))
                    .overflow_hidden()
                    .bg(rgb(0x19191d))
                    .child(
                        img(Arc::clone(image))
                            .size_full()
                            .object_fit(ObjectFit::Contain),
                    ),
            );
        }
        if let Some(text) = original {
            content = content.child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex_shrink_0()
                    .whitespace_normal()
                    .text_color(rgb(INK))
                    .text_size(px(13.))
                    .line_height(px(19.))
                    .child(text.clone()),
            );
        }
        let mut actions = div().flex_shrink_0().flex().justify_end().gap(px(8.));
        if original.is_some() {
            let label = if result.copied {
                labels.copied
            } else {
                labels.copy
            };
            actions = actions.child(
                button("share-result-copy", label, false)
                    .h(px(28.))
                    .px(px(12.))
                    .rounded(px(8.))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(result) = &mut this.share_result
                            && let Some(original) = &result.original
                        {
                            cx.write_to_clipboard(ClipboardItem::new_string(original.clone()));
                            result.copied = true;
                            cx.notify();
                        }
                    })),
            );
        }
        actions = actions.child(
            button("share-result-done", labels.done, true)
                .h(px(28.))
                .px(px(12.))
                .rounded(px(8.))
                .on_click(cx.listener(|this, _, window, cx| this.finish_share_result(window, cx))),
        );
        div()
            .w_full()
            .min_h_0()
            .flex_1()
            .flex()
            .flex_col()
            .gap(px(10.))
            .rounded(px(14.))
            .p(px(14.))
            .bg(rgb(SURFACE))
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(rgb(INK))
                    .text_size(px(16.))
                    .line_height(px(21.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(result.title.clone()),
            )
            .child(content)
            .child(actions)
            .into_any_element()
    }
}
