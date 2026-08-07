use super::{
    App, CachedFrameTitle, ChromeText, ChromeTextStyle, Context, DojoId, HashMap, HashSet,
    LayoutNode, PaneView, Receiver, Rect, RenderContext, ResolvedTheme, Result, Sender, SplintId,
    Waker, WindowDojoIdentity, WindowPaneOptions, WindowTabSet, WindowTopologyCommand,
    WindowTopologyUpdate, WindowUpdate, apply_theme, drain_receiver, fill_rect,
    logical_extent_to_buffer, pane_stream_has_terminal_notice, rect_contains, sanitized_tab_label,
};

pub(super) const TAB_STRIP_LOGICAL_HEIGHT: u32 = 34;
const TAB_PREFERRED_LOGICAL_WIDTH: u32 = 180;
const TAB_MIN_LOGICAL_WIDTH: u32 = 96;
const TAB_ACTION_LOGICAL_WIDTH: u32 = 34;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TabHitTarget {
    Activate(DojoId),
    Close(DojoId),
    New,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct VisibleTabLayout {
    pub(super) dojo_id: DojoId,
    pub(super) rect: Rect,
    pub(super) label_rect: Rect,
    pub(super) close_rect: Rect,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TabStripLayout {
    pub(super) rect: Rect,
    pub(super) tabs: Vec<VisibleTabLayout>,
    pub(super) new_rect: Rect,
}

pub(super) fn tab_strip_layout(
    width: u32,
    dojo_ids: &[DojoId],
    active: usize,
) -> Option<TabStripLayout> {
    if width == 0 || dojo_ids.is_empty() || active >= dojo_ids.len() {
        return None;
    }
    let new_width = TAB_ACTION_LOGICAL_WIDTH.min((width / 4).max(u32::from(width > 0)));
    let tab_space = width.saturating_sub(new_width);
    let capacity = usize::try_from((tab_space / TAB_MIN_LOGICAL_WIDTH).max(1)).unwrap_or(1);
    let visible_count = dojo_ids.len().min(capacity);
    let mut start = active.saturating_sub(visible_count / 2);
    start = start.min(dojo_ids.len().saturating_sub(visible_count));
    let tab_width = if visible_count == 0 {
        0
    } else {
        (tab_space / u32::try_from(visible_count).unwrap_or(1)).min(TAB_PREFERRED_LOGICAL_WIDTH)
    };
    let mut tabs = Vec::with_capacity(visible_count);
    let mut x = 0;
    for dojo_id in dojo_ids.iter().skip(start).take(visible_count).copied() {
        let remaining = tab_space.saturating_sub(x);
        let width = tab_width.min(remaining);
        let close_width = TAB_ACTION_LOGICAL_WIDTH.min((width / 3).max(u32::from(width > 1)));
        let label_space = width.saturating_sub(close_width);
        let label_inset = 10_u32.min(label_space / 4);
        tabs.push(VisibleTabLayout {
            dojo_id,
            rect: Rect {
                x,
                y: 0,
                width,
                height: TAB_STRIP_LOGICAL_HEIGHT,
            },
            label_rect: Rect {
                x: x.saturating_add(label_inset),
                y: 0,
                width: label_space.saturating_sub(label_inset.saturating_mul(2)),
                height: TAB_STRIP_LOGICAL_HEIGHT,
            },
            close_rect: Rect {
                x: x.saturating_add(width.saturating_sub(close_width)),
                y: 0,
                width: close_width,
                height: TAB_STRIP_LOGICAL_HEIGHT,
            },
        });
        x = x.saturating_add(width);
    }
    Some(TabStripLayout {
        rect: Rect {
            x: 0,
            y: 0,
            width,
            height: TAB_STRIP_LOGICAL_HEIGHT,
        },
        tabs,
        new_rect: Rect {
            x: width.saturating_sub(new_width),
            y: 0,
            width: new_width,
            height: TAB_STRIP_LOGICAL_HEIGHT,
        },
    })
}

pub(super) const fn tab_context_target(target: TabHitTarget) -> Option<DojoId> {
    match target {
        TabHitTarget::Activate(dojo_id) | TabHitTarget::Close(dojo_id) => Some(dojo_id),
        TabHitTarget::New => None,
    }
}

const fn tab_foreground(theme: ResolvedTheme, active: bool) -> u32 {
    if active {
        theme.selection_foreground
    } else {
        theme.foreground
    }
}

pub(super) fn tab_strip_hit_test(
    layout: &TabStripLayout,
    position: (f64, f64),
) -> Option<TabHitTarget> {
    if !rect_contains(layout.rect, position) {
        return None;
    }
    if rect_contains(layout.new_rect, position) {
        return Some(TabHitTarget::New);
    }
    layout.tabs.iter().find_map(|tab| {
        if rect_contains(tab.close_rect, position) {
            Some(TabHitTarget::Close(tab.dojo_id))
        } else if rect_contains(tab.rect, position) {
            Some(TabHitTarget::Activate(tab.dojo_id))
        } else {
            None
        }
    })
}

pub(super) struct DojoTabView {
    pub(super) identity: WindowDojoIdentity,
    pub(super) pane: PaneView,
    pub(super) inactive_panes: Vec<PaneView>,
    pub(super) layout: Option<LayoutNode>,
    pub(super) pending_exited_splints: HashSet<SplintId>,
    pub(super) frame_titles: HashMap<SplintId, CachedFrameTitle>,
    pub(super) dirty_inactive_panes: HashSet<SplintId>,
}

impl DojoTabView {
    pub(super) fn focused_splint(&self) -> Option<SplintId> {
        self.pane
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.splint_id)
    }

    pub(super) fn focus_splint(&mut self, splint_id: SplintId) -> bool {
        if self.focused_splint() == Some(splint_id) {
            return true;
        }
        let Some(index) = self.inactive_panes.iter().position(|pane| {
            pane.snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.splint_id == splint_id)
        }) else {
            return false;
        };
        std::mem::swap(&mut self.pane, &mut self.inactive_panes[index]);
        true
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn apply_topology(
        &mut self,
        layout: LayoutNode,
        added: Vec<WindowPaneOptions>,
        removed: Vec<SplintId>,
        focused: Option<SplintId>,
        theme: ResolvedTheme,
        scale_120: u32,
        context: &RenderContext,
    ) -> Result<()> {
        let removed = removed.into_iter().collect::<HashSet<_>>();
        let mut prepared = Vec::with_capacity(added.len());
        for mut pane in added {
            apply_theme(&mut pane.snapshot, theme);
            prepared.push(PaneView::from_inactive_options_with_context(
                pane, scale_120, context,
            )?);
        }
        let prepared_ids = prepared
            .iter()
            .filter_map(|pane| pane.snapshot.as_ref().map(|snapshot| snapshot.splint_id));
        let mut identities = std::iter::once(&self.pane)
            .chain(self.inactive_panes.iter())
            .filter_map(|pane| pane.snapshot.as_ref().map(|snapshot| snapshot.splint_id))
            .filter(|splint_id| !removed.contains(splint_id))
            .chain(prepared_ids)
            .collect::<HashSet<_>>();
        anyhow::ensure!(
            identities.len() == layout.splint_count()
                && identities
                    .iter()
                    .all(|splint_id| layout.find_splint(*splint_id).is_some()),
            "topology update pane identities do not match its layout"
        );
        let next_focus = focused.or_else(|| {
            self.focused_splint()
                .filter(|splint_id| !removed.contains(splint_id))
        });
        let next_focus = next_focus.unwrap_or_else(|| layout.first_splint_id());
        anyhow::ensure!(
            identities.remove(&next_focus),
            "topology update focus is absent"
        );

        self.pending_exited_splints
            .retain(|splint_id| !removed.contains(splint_id));
        self.inactive_panes.extend(prepared);
        let focused = self.focus_splint(next_focus);
        debug_assert!(focused);
        self.inactive_panes.retain(|pane| {
            pane.snapshot
                .as_ref()
                .is_none_or(|snapshot| !removed.contains(&snapshot.splint_id))
        });
        self.layout = Some(layout);
        Ok(())
    }

    pub(super) fn drain_hidden_updates(
        &mut self,
        waker: &Waker,
        theme: ResolvedTheme,
    ) -> Result<()> {
        for pane in std::iter::once(&mut self.pane).chain(self.inactive_panes.iter_mut()) {
            let mut pending = Vec::new();
            let mut disconnected = false;
            if let Some(updates) = &mut pane.updates {
                let drained = drain_receiver(updates, waker);
                pending = drained.items;
                disconnected = drained.disconnected;
            }
            let terminal_notice = pane_stream_has_terminal_notice(&pending);
            if disconnected && !terminal_notice {
                anyhow::bail!("hidden pane update stream disconnected unexpectedly");
            }
            for update in pending {
                if let WindowUpdate::Exited { splint_id } = update {
                    self.pending_exited_splints.insert(splint_id);
                    let impact = pane.apply_background_update(
                        WindowUpdate::Exited { splint_id },
                        theme,
                        "hidden",
                    )?;
                    if impact.frame_dirty {
                        self.dirty_inactive_panes.insert(splint_id);
                    }
                    continue;
                }
                if matches!(update, WindowUpdate::Theme(_)) {
                    continue;
                }
                let splint_id = pane.snapshot.as_ref().map(|snapshot| snapshot.splint_id);
                let impact = pane.apply_background_update(update, theme, "hidden")?;
                if impact.frame_dirty
                    && let Some(splint_id) = splint_id
                {
                    self.dirty_inactive_panes.insert(splint_id);
                }
            }
        }
        Ok(())
    }

    pub(super) fn from_open(
        identity: WindowDojoIdentity,
        layout: LayoutNode,
        mut panes: Vec<WindowPaneOptions>,
        focused: SplintId,
        theme: ResolvedTheme,
        scale_120: u32,
        context: &RenderContext,
    ) -> Result<Self> {
        anyhow::ensure!(
            layout.find_splint(focused).is_some() && layout.splint_count() == panes.len(),
            "opened tab panes do not match its layout"
        );
        for pane in &mut panes {
            apply_theme(&mut pane.snapshot, theme);
        }
        let active_index = panes
            .iter()
            .position(|pane| pane.snapshot.splint_id == focused)
            .context("opened tab focus is absent from its panes")?;
        let active = panes.remove(active_index);
        let pane = PaneView::from_options_with_context(active, scale_120, context)?;
        let inactive_panes = panes
            .into_iter()
            .map(|pane| PaneView::from_inactive_options_with_context(pane, scale_120, context))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            identity,
            pane,
            inactive_panes,
            layout: Some(layout),
            pending_exited_splints: HashSet::new(),
            frame_titles: HashMap::new(),
            dirty_inactive_panes: HashSet::new(),
        })
    }
}

pub(super) struct TabsState {
    pub(super) tabs: WindowTabSet<Option<DojoTabView>>,
    pub(super) active_identity: WindowDojoIdentity,
    pub(super) managed_tabs: bool,
    pub(super) tab_strip_layout: Option<TabStripLayout>,
    pub(super) tab_strip_pressed: Option<(u32, TabHitTarget)>,
    pub(super) tab_label_cache: HashMap<DojoId, CachedFrameTitle>,
    pub(super) tab_close_text: Option<(u32, ChromeText)>,
    pub(super) tab_new_text: Option<(u32, ChromeText)>,
    pub(super) topology_updates: Option<Receiver<WindowTopologyUpdate>>,
    pub(super) topology_commands: Option<Sender<WindowTopologyCommand>>,
    pub(super) session_switch_pending: bool,
    pub(super) deferred_topology_updates: Vec<WindowTopologyUpdate>,
}

impl TabsState {
    pub(super) const fn active_dojo_id(&self) -> DojoId {
        self.active_identity.dojo_id
    }

    pub(super) fn tab_identity(&self, dojo_id: DojoId) -> Option<&WindowDojoIdentity> {
        if dojo_id == self.active_dojo_id() {
            return Some(&self.active_identity);
        }
        self.tabs
            .get(dojo_id)
            .and_then(|tab| tab.value.as_ref())
            .map(|view| &view.identity)
    }

    pub(super) fn tab_label(&self, dojo_id: DojoId) -> String {
        let Some(identity) = self.tab_identity(dojo_id) else {
            return "Untitled Dojo".to_owned();
        };
        let dojo_label = sanitized_tab_label(&identity.dojo_name, 128, 48);
        let ambiguous = self.tabs.iter().filter(|tab| {
            self.tab_identity(tab.dojo_id).is_some_and(|candidate| {
                sanitized_tab_label(&candidate.dojo_name, 128, 48) == dojo_label
            })
        });
        if ambiguous.count() > 1 {
            sanitized_tab_label(
                &format!("{} / {}", identity.lair_name, identity.dojo_name),
                128,
                48,
            )
        } else {
            dojo_label
        }
    }
}

impl App {
    pub(super) fn current_tab_strip_layout(&self) -> Option<TabStripLayout> {
        if !self.tab_state.managed_tabs {
            return None;
        }
        let ids = self
            .tab_state
            .tabs
            .iter()
            .map(|tab| tab.dojo_id)
            .collect::<Vec<_>>();
        let active = ids
            .iter()
            .position(|dojo_id| *dojo_id == self.tab_state.active_dojo_id())?;
        tab_strip_layout(self.surface.logical_width, &ids, active)
    }

    pub(super) fn prepare_tab_strip_text(&mut self, layout: &TabStripLayout) -> Result<()> {
        let visible = layout
            .tabs
            .iter()
            .map(|tab| tab.dojo_id)
            .collect::<HashSet<_>>();
        self.tab_state
            .tab_label_cache
            .retain(|dojo_id, _| visible.contains(dojo_id));
        for tab in &layout.tabs {
            let source = self.tab_state.tab_label(tab.dojo_id);
            let maximum_cells = (tab.label_rect.width / 8).max(1);
            let active = tab.dojo_id == self.tab_state.active_dojo_id();
            let current = self
                .tab_state
                .tab_label_cache
                .get(&tab.dojo_id)
                .is_some_and(|cached| {
                    cached.source == source
                        && cached.maximum_cells == maximum_cells
                        && cached.scale_120 == self.surface.scale_120
                        && cached.bold == active
                });
            if !current {
                let style = if active {
                    ChromeTextStyle::Bold
                } else {
                    ChromeTextStyle::Regular
                };
                self.tab_state.tab_label_cache.insert(
                    tab.dojo_id,
                    CachedFrameTitle {
                        text: ChromeText::load_styled_with_context(
                            &source,
                            self.surface.scale_120,
                            style,
                            &self.presentation.render_context,
                        )?,
                        source,
                        maximum_cells,
                        scale_120: self.surface.scale_120,
                        bold: active,
                    },
                );
            }
        }
        if self
            .tab_state
            .tab_close_text
            .as_ref()
            .is_none_or(|(scale, _)| *scale != self.surface.scale_120)
        {
            self.tab_state.tab_close_text = Some((
                self.surface.scale_120,
                ChromeText::load_with_context(
                    "×",
                    self.surface.scale_120,
                    &self.presentation.render_context,
                )?,
            ));
            self.tab_state.tab_new_text = Some((
                self.surface.scale_120,
                ChromeText::load_with_context(
                    "+",
                    self.surface.scale_120,
                    &self.presentation.render_context,
                )?,
            ));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn paint_tab_strip(
        canvas: &mut [u8],
        width: u32,
        height: u32,
        layout: &TabStripLayout,
        scale_120: u32,
        theme: ResolvedTheme,
        active_dojo: DojoId,
        labels: &HashMap<DojoId, CachedFrameTitle>,
        close_text: Option<&ChromeText>,
        new_text: Option<&ChromeText>,
    ) -> Result<()> {
        let rgba = |color: u32| {
            [
                u8::try_from((color >> 16) & 0xff).unwrap_or(0),
                u8::try_from((color >> 8) & 0xff).unwrap_or(0),
                u8::try_from(color & 0xff).unwrap_or(0),
                u8::MAX,
            ]
        };
        let position = |value| i32::try_from(value).unwrap_or(i32::MAX);
        let strip = Self::buffer_rect(layout.rect, scale_120)?;
        fill_rect(
            canvas,
            width,
            height,
            (
                position(strip.x),
                position(strip.y),
                strip.width,
                strip.height,
            ),
            rgba(theme.background),
        );
        for tab in &layout.tabs {
            let rect = Self::buffer_rect(tab.rect, scale_120)?;
            let active = tab.dojo_id == active_dojo;
            let foreground = tab_foreground(theme, active);
            if active {
                fill_rect(
                    canvas,
                    width,
                    height,
                    (position(rect.x), position(rect.y), rect.width, rect.height),
                    rgba(theme.selection),
                );
                let underline = logical_extent_to_buffer(3, scale_120)?.max(1);
                fill_rect(
                    canvas,
                    width,
                    height,
                    (
                        position(rect.x),
                        position(rect.y.saturating_add(rect.height.saturating_sub(underline))),
                        rect.width,
                        underline,
                    ),
                    rgba(theme.ui_accent),
                );
            }
            let label_rect = Self::buffer_rect(tab.label_rect, scale_120)?;
            if let Some(label) = labels.get(&tab.dojo_id) {
                let y = label_rect.y.saturating_add(
                    label_rect.height.saturating_sub(label.text.pixel_height()) / 2,
                );
                label.text.paint(
                    canvas,
                    width,
                    height,
                    (label_rect.x, y),
                    label_rect,
                    foreground,
                );
            }
            let close = Self::buffer_rect(tab.close_rect, scale_120)?;
            if let Some(text) = close_text {
                text.paint(
                    canvas,
                    width,
                    height,
                    (
                        close
                            .x
                            .saturating_add(close.width.saturating_sub(text.pixel_width()) / 2),
                        close
                            .y
                            .saturating_add(close.height.saturating_sub(text.pixel_height()) / 2),
                    ),
                    close,
                    foreground,
                );
            }
        }
        let new_rect = Self::buffer_rect(layout.new_rect, scale_120)?;
        if let Some(text) = new_text {
            text.paint(
                canvas,
                width,
                height,
                (
                    new_rect
                        .x
                        .saturating_add(new_rect.width.saturating_sub(text.pixel_width()) / 2),
                    new_rect
                        .y
                        .saturating_add(new_rect.height.saturating_sub(text.pixel_height()) / 2),
                ),
                new_rect,
                theme.ui_accent,
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DojoId, ResolvedTheme, TabHitTarget, tab_context_target, tab_foreground,
        tab_strip_hit_test, tab_strip_layout,
    };

    #[test]
    fn active_tab_uses_selection_foreground() {
        let theme = ResolvedTheme {
            foreground: 0xaa_bb_cc,
            selection_foreground: 0x11_22_33,
            ..ResolvedTheme::default()
        };
        assert_eq!(tab_foreground(theme, false), 0xaa_bb_cc);
        assert_eq!(tab_foreground(theme, true), 0x11_22_33);
    }

    #[test]
    fn right_click_target_covers_the_tab_body_and_close_affordance_only() {
        let dojo_id = DojoId::new();
        assert_eq!(
            tab_context_target(TabHitTarget::Activate(dojo_id)),
            Some(dojo_id)
        );
        assert_eq!(
            tab_context_target(TabHitTarget::Close(dojo_id)),
            Some(dojo_id)
        );
        assert_eq!(tab_context_target(TabHitTarget::New), None);
    }

    #[test]
    fn tab_strip_layout_keeps_active_visible_with_bounded_non_overlapping_targets() {
        let ids = (0..12).map(|_| DojoId::new()).collect::<Vec<_>>();
        let layout = tab_strip_layout(420, &ids, 10).unwrap();
        assert!(layout.tabs.iter().any(|tab| tab.dojo_id == ids[10]));
        assert!(layout.tabs.len() < ids.len());
        assert!(layout.tabs.iter().all(|tab| {
            tab.rect.x.saturating_add(tab.rect.width) <= layout.new_rect.x
                && tab.label_rect.x.saturating_add(tab.label_rect.width) <= tab.close_rect.x
        }));
        for pair in layout.tabs.windows(2) {
            assert!(pair[0].rect.x.saturating_add(pair[0].rect.width) <= pair[1].rect.x);
        }
        let close = layout.tabs[0].close_rect;
        assert_eq!(
            tab_strip_hit_test(&layout, (f64::from(close.x), f64::from(close.y)),),
            Some(TabHitTarget::Close(layout.tabs[0].dojo_id))
        );
    }

    #[test]
    fn tab_strip_compact_layout_retains_active_close_and_new_targets() {
        let ids = [DojoId::new(), DojoId::new(), DojoId::new()];
        let layout = tab_strip_layout(80, &ids, 1).unwrap();
        assert_eq!(layout.tabs.len(), 1);
        assert_eq!(layout.tabs[0].dojo_id, ids[1]);
        assert!(layout.tabs[0].label_rect.width > 0);
        assert!(layout.tabs[0].close_rect.width > 0);
        assert!(layout.new_rect.width > 0);
        for width in [3, 35] {
            let compact = tab_strip_layout(width, &ids, 1).unwrap();
            assert!(compact.tabs[0].label_rect.width > 0);
            assert!(compact.tabs[0].close_rect.width > 0);
            assert!(compact.new_rect.width > 0);
        }
    }
}
