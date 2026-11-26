use gpui::{
    AnyElement, App, FontWeight, IntoElement, ParentElement as _, SharedString, Styled as _, div,
    px, relative,
};
use moonglass::theme::ActiveTheme as _;
use moonglass::{IconName, h_flex, v_flex};
use riven_play::PlayError;
use rust_i18n::t;

/// What went wrong with BX Team, in the player's language: the server's `code` picks `errors.<CODE>`,
/// its `params` fill the placeholders, so a new server code needs only the two locale lines.
pub fn play_error(e: &PlayError) -> String {
    match e {
        PlayError::Network(_) => return t!("errors.NETWORK").into(),
        PlayError::SignedOut => return t!("errors.UNAUTHORIZED").into(),
        PlayError::Expired => return t!("bxid.failed.expired").into(),
        PlayError::Denied => return t!("bxid.failed.denied").into(),
        PlayError::Connect(_) => return t!("errors.CONNECT").into(),
        PlayError::Decode(_) => return t!("errors.GENERIC").into(),
        _ => {}
    }
    let Some(problem) = e.problem().filter(|p| !p.code.is_empty()) else {
        return e.to_string();
    };
    let key = format!("errors.{}", problem.code);
    let Some(text) = crate::_rust_i18n_try_translate(&rust_i18n::locale(), &key) else {
        tracing::warn!(
            "no text for BX Team error {}: {}",
            problem.code,
            problem.message
        );
        return t!("errors.GENERIC").into();
    };
    problem
        .params
        .iter()
        .fold(text.into_owned(), |text, (name, value)| {
            let value = match value {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            text.replace(&format!("%{{{name}}}"), &value)
        })
}

/// The warning mark both shapes share: a small tinted circle with an exclamation.
fn mark(size: f32, cx: &App) -> impl IntoElement {
    let c = cx.theme().colors;
    div()
        .flex_none()
        .size(px(size))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(c.warn.opacity(0.14))
        .child(moonglass::icon(IconName::Alert, c.warn).size(px(size * 0.54)))
}

/// Why something stopped, on a quiet card: a short title and what to do about it.
pub fn card((title, detail): &(SharedString, SharedString), cx: &App) -> AnyElement {
    let c = cx.theme().colors;
    h_flex()
        .items_start()
        .gap(px(12.))
        .p(px(14.))
        .rounded(px(10.))
        .bg(c.bg)
        .border_1()
        .border_color(c.border)
        .child(mark(28., cx))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(3.))
                .child(div().font_weight(FontWeight::SEMIBOLD).child(title.clone()))
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(c.muted)
                        .line_height(relative(1.5))
                        .child(detail.clone()),
                ),
        )
        .into_any_element()
}

/// A one-line refusal by a field or in a panel, in the same look as [`card`].
pub fn inline(text: impl Into<SharedString>, cx: &App) -> gpui::Div {
    let c = cx.theme().colors;
    h_flex()
        .items_start()
        .gap(px(8.))
        .px(px(10.))
        .py(px(8.))
        .rounded(px(8.))
        .bg(c.bg)
        .border_1()
        .border_color(c.border)
        .child(mark(18., cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .pt(px(1.))
                .text_size(px(12.))
                .text_color(c.text2)
                .line_height(relative(1.4))
                .child(text.into()),
        )
}

#[cfg(test)]
mod tests {
    use riven_play::{PlayError, Problem};

    #[test]
    fn fills_numeric_params_from_the_server() {
        let mut params = serde_json::Map::new();
        params.insert("max".into(), serde_json::json!(100));
        let e = PlayError::Refused(Problem {
            code: "FRIEND_LIMIT".into(),
            message: "A BX ID has at most 100 friends".into(),
            params,
        });
        let text = super::play_error(&e);
        assert!(text.contains("100") && !text.contains("%{"), "{text}");
    }
}
