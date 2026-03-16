use fuzzy::{StringMatch, StringMatchCandidate, match_strings};
use gpui::{
    AnyElement, App, BackgroundExecutor, Context, DismissEvent, Entity, FocusHandle, Focusable,
    SharedString, Task, Window,
};
use picker::{Picker, PickerDelegate, popover_menu::PickerPopoverMenu};
use std::sync::{Arc, atomic::AtomicBool, atomic::Ordering};
use ui::{HighlightedLabel, LabelSize, ListItem, ListItemSpacing, PopoverMenuHandle, TintColor, Tooltip, prelude::*};

/// Trait for types that can provide and manage agent personas.
pub trait PersonaProvider {
    /// Get the current persona name (None if no persona selected).
    fn persona_name(&self, cx: &App) -> Option<String>;
    /// Set the active persona by name (None clears the persona).
    fn set_persona(&self, name: Option<String>, cx: &mut App);
    /// List available personas as (name, description) pairs.
    fn available_personas(&self, cx: &App) -> Vec<(String, String)>;
}

pub struct PersonaSelector {
    personas: Vec<(String, String)>,
    provider: Arc<dyn PersonaProvider>,
    picker: Option<Entity<Picker<PersonaPickerDelegate>>>,
    picker_handle: PopoverMenuHandle<Picker<PersonaPickerDelegate>>,
    focus_handle: FocusHandle,
}

impl PersonaSelector {
    pub fn new(
        provider: Arc<dyn PersonaProvider>,
        focus_handle: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let personas = provider.available_personas(cx);
        Self {
            personas,
            provider,
            picker: None,
            picker_handle: PopoverMenuHandle::default(),
            focus_handle,
        }
    }

    pub fn menu_handle(&self) -> PopoverMenuHandle<Picker<PersonaPickerDelegate>> {
        self.picker_handle.clone()
    }

    fn ensure_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Picker<PersonaPickerDelegate>> {
        if self.picker.is_none() {
            let delegate = PersonaPickerDelegate::new(
                self.provider.clone(),
                self.personas.clone(),
                cx.background_executor().clone(),
                cx,
            );
            let picker = cx.new(|cx| {
                Picker::list(delegate, window, cx)
                    .show_scrollbar(true)
                    .width(rems(18.))
                    .max_height(Some(rems(20.).into()))
            });
            self.picker = Some(picker);
        }
        self.picker.as_ref().unwrap().clone()
    }
}

impl Focusable for PersonaSelector {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        if let Some(picker) = &self.picker {
            picker.focus_handle(cx)
        } else {
            self.focus_handle.clone()
        }
    }
}

impl Render for PersonaSelector {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let picker = self.ensure_picker(window, cx);

        let current = self.provider.persona_name(cx);
        let label: SharedString = current
            .clone()
            .map(|n| n.into())
            .unwrap_or_else(|| "No Persona".into());

        let icon = if self.picker_handle.is_deployed() {
            IconName::ChevronUp
        } else {
            IconName::ChevronDown
        };

        let trigger_button = Button::new("persona-selector", label)
            .label_size(LabelSize::Small)
            .color(Color::Muted)
            .icon(icon)
            .icon_size(IconSize::XSmall)
            .icon_position(IconPosition::End)
            .icon_color(Color::Muted)
            .selected_style(ButtonStyle::Tinted(TintColor::Accent));

        PickerPopoverMenu::new(
            picker,
            trigger_button,
            Tooltip::text("Change Persona"),
            gpui::Corner::BottomRight,
            cx,
        )
        .with_handle(self.picker_handle.clone())
        .render(window, cx)
        .into_any_element()
    }
}

#[derive(Clone)]
struct PersonaCandidate {
    /// None means "No Persona"
    name: Option<String>,
    description: String,
}

#[derive(Clone)]
struct PersonaMatchEntry {
    candidate_index: usize,
    positions: Vec<usize>,
}

pub struct PersonaPickerDelegate {
    provider: Arc<dyn PersonaProvider>,
    background: BackgroundExecutor,
    candidates: Vec<PersonaCandidate>,
    string_candidates: Arc<Vec<StringMatchCandidate>>,
    filtered_entries: Vec<PersonaMatchEntry>,
    selected_index: usize,
    query: String,
    cancel: Option<Arc<AtomicBool>>,
}

impl PersonaPickerDelegate {
    fn new(
        provider: Arc<dyn PersonaProvider>,
        personas: Vec<(String, String)>,
        background: BackgroundExecutor,
        cx: &mut Context<PersonaSelector>,
    ) -> Self {
        // First entry is always "No Persona"
        let mut candidates = vec![PersonaCandidate {
            name: None,
            description: "Clear persona — run without constraints".to_string(),
        }];
        for (name, description) in personas {
            candidates.push(PersonaCandidate {
                name: Some(name),
                description,
            });
        }

        let string_candidates = Arc::new(Self::build_string_candidates(&candidates));
        let filtered_entries = Self::all_entries(candidates.len());

        let current = provider.persona_name(cx);
        let selected_index = candidates
            .iter()
            .position(|c| c.name == current)
            .unwrap_or(0);

        Self {
            provider,
            background,
            candidates,
            string_candidates,
            filtered_entries,
            selected_index,
            query: String::new(),
            cancel: None,
        }
    }

    fn build_string_candidates(candidates: &[PersonaCandidate]) -> Vec<StringMatchCandidate> {
        candidates
            .iter()
            .enumerate()
            .map(|(i, c)| {
                StringMatchCandidate::new(
                    i,
                    c.name.as_deref().unwrap_or("No Persona"),
                )
            })
            .collect()
    }

    fn all_entries(count: usize) -> Vec<PersonaMatchEntry> {
        (0..count)
            .map(|i| PersonaMatchEntry {
                candidate_index: i,
                positions: Vec::new(),
            })
            .collect()
    }

    fn entries_from_matches(matches: Vec<StringMatch>) -> Vec<PersonaMatchEntry> {
        matches
            .into_iter()
            .map(|m| PersonaMatchEntry {
                candidate_index: m.candidate_id,
                positions: m.positions,
            })
            .collect()
    }

}

impl PickerDelegate for PersonaPickerDelegate {
    type ListItem = AnyElement;

    fn placeholder_text(&self, _: &mut Window, _: &mut App) -> Arc<str> {
        "Search personas…".into()
    }

    fn no_matches_text(&self, _: &mut Window, _: &mut App) -> Option<SharedString> {
        Some(if self.candidates.len() <= 1 {
            "No personas found. Create .prism/personas/<name>.toml to get started.".into()
        } else {
            "No personas match your search.".into()
        })
    }

    fn match_count(&self) -> usize {
        self.filtered_entries.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(&mut self, ix: usize, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        self.selected_index = ix.min(self.filtered_entries.len().saturating_sub(1));
        cx.notify();
    }

    fn update_matches(
        &mut self,
        query: String,
        _window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        if query.is_empty() {
            self.query.clear();
            self.filtered_entries = Self::all_entries(self.candidates.len());
            let current = self.provider.persona_name(cx);
            self.selected_index = self
                .candidates
                .iter()
                .position(|c| c.name == current)
                .unwrap_or(0);
            cx.notify();
            return Task::ready(());
        }

        if let Some(prev) = &self.cancel {
            prev.store(true, Ordering::Relaxed);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());

        let string_candidates = self.string_candidates.clone();
        let background = self.background.clone();
        self.query = query.clone();

        cx.spawn_in(_window, async move |this, cx| {
            let matches = match_strings(
                &string_candidates,
                &query,
                false,
                true,
                100,
                cancel.as_ref(),
                background,
            )
            .await;

            this.update_in(cx, |this, _, cx| {
                if this.delegate.query != query {
                    return;
                }
                this.delegate.filtered_entries = Self::entries_from_matches(matches);
                let current = this.delegate.provider.persona_name(cx);
                this.delegate.selected_index = this
                    .delegate
                    .filtered_entries
                    .iter()
                    .position(|e| {
                        this.delegate
                            .candidates
                            .get(e.candidate_index)
                            .map(|c| c.name == current)
                            .unwrap_or(false)
                    })
                    .unwrap_or(0);
                cx.notify();
            })
            .ok();
        })
    }

    fn confirm(&mut self, _: bool, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        if let Some(entry) = self.filtered_entries.get(self.selected_index) {
            if let Some(candidate) = self.candidates.get(entry.candidate_index) {
                let name = candidate.name.clone();
                self.provider.set_persona(name, cx);
            }
        }
        cx.emit(DismissEvent);
    }

    fn dismissed(&mut self, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        cx.defer_in(window, |picker, window, cx| {
            picker.set_query("", window, cx);
        });
        cx.emit(DismissEvent);
    }

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let entry = self.filtered_entries.get(ix)?;
        let candidate = self.candidates.get(entry.candidate_index)?;

        let current = self.provider.persona_name(cx);
        let is_active = candidate.name == current;

        let display_name: SharedString = candidate
            .name
            .clone()
            .map(|n| n.into())
            .unwrap_or_else(|| "No Persona".into());
        let description: SharedString = candidate.description.clone().into();

        Some(
            div()
                .child(
                    ListItem::new(("persona-item", ix))
                        .inset(true)
                        .spacing(ListItemSpacing::Sparse)
                        .toggle_state(selected)
                        .child(
                            v_flex()
                                .gap_0p5()
                                .child(HighlightedLabel::new(display_name, entry.positions.clone()))
                                .child(
                                    Label::new(description)
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                ),
                        )
                        .when(is_active, |this| {
                            this.end_slot(
                                div()
                                    .pr_2()
                                    .child(Icon::new(IconName::Check).color(Color::Accent)),
                            )
                        }),
                )
                .into_any_element(),
        )
    }
}
