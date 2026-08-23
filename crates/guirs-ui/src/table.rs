//! Rows and columns, with headers that sort and edges that drag.
//!
//! The table does not hold the data and does not sort it. It reports which
//! column somebody asked to sort by and in which direction, and the
//! application sorts its own rows. Anything else means copying the data in, or
//! teaching the table how to compare values it has never seen.
//!
//! ```
//! # use guirs_ui::{table::{table, table_column, TableState}, div::div, text::text, Model};
//! let state = Model::new(TableState::default());
//! let columns = [table_column("name", "Name").width(220.0), table_column("size", "Size")];
//! # let rows: Vec<(String, String)> = Vec::new();
//! let count = rows.len();
//! table(&columns, count, state, move |row, col| {
//!     div().child(text(match col.key().as_ref() {
//!         "name" => rows[row].0.clone(),
//!         _ => rows[row].1.clone(),
//!     }))
//! });
//! ```

use std::collections::HashMap;
use std::rc::Rc;

use guirs_core::{Px, SharedString};
use guirs_style::{CursorStyle, StateFlags, Styled};

use crate::div::{div, Div};
use crate::model::Model;
use crate::text::text;

/// The narrowest a column can be dragged, in logical pixels.
///
/// Not zero: a column dragged shut cannot be found again, because the handle
/// that would widen it has gone with it.
pub const MIN_COLUMN: f32 = 40.0;

/// How wide the grab area on a column's edge is.
pub const EDGE_HIT: f32 = 6.0;

/// Which way a column is sorted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortOrder {
    Ascending,
    Descending,
}

impl SortOrder {
    pub fn arrow(self) -> &'static str {
        match self {
            SortOrder::Ascending => "\u{25b4}",
            SortOrder::Descending => "\u{25be}",
        }
    }
}

/// One column.
#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    key: SharedString,
    title: SharedString,
    width: f32,
    sortable: bool,
    resizable: bool,
}

/// A column, identified by a key the application recognises.
pub fn table_column(key: impl Into<SharedString>, title: impl Into<SharedString>) -> Column {
    Column {
        key: key.into(),
        title: title.into(),
        width: 140.0,
        sortable: true,
        resizable: true,
    }
}

impl Column {
    pub fn width(mut self, width: f32) -> Self {
        self.width = width.max(MIN_COLUMN);
        self
    }

    /// Whether clicking the header sorts by this column.
    pub fn sortable(mut self, sortable: bool) -> Self {
        self.sortable = sortable;
        self
    }

    /// Whether this column's edge can be dragged.
    pub fn resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    pub fn key(&self) -> &SharedString {
        &self.key
    }

    pub fn title(&self) -> &SharedString {
        &self.title
    }

    /// The width it was given, before anything was dragged.
    pub fn default_width(&self) -> f32 {
        self.width
    }
}

/// What has been sorted, resized and chosen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TableState {
    /// Widths somebody has dragged, by column key. Anything not here keeps
    /// the width its column was declared with.
    widths: HashMap<SharedString, f32>,
    sort: Option<(SharedString, SortOrder)>,
    selected: Option<usize>,
    /// The column being resized, where the pointer was when it started, and
    /// how wide the column was then.
    grab: Option<(SharedString, f32, f32)>,
}

impl TableState {
    /// A table already sorted by a column.
    pub fn sorted_by(key: impl Into<SharedString>, order: SortOrder) -> Self {
        TableState {
            sort: Some((key.into(), order)),
            ..Default::default()
        }
    }

    /// Which column is sorted, and which way.
    pub fn sort(&self) -> Option<(&SharedString, SortOrder)> {
        self.sort.as_ref().map(|(key, order)| (key, *order))
    }

    /// The direction a column is sorted, if it is the sorted one.
    pub fn order_of(&self, key: &SharedString) -> Option<SortOrder> {
        match &self.sort {
            Some((sorted, order)) if sorted == key => Some(*order),
            _ => None,
        }
    }

    /// Ask to sort by a column, the way clicking its header does.
    ///
    /// Ascending, then descending, then not sorted at all. The third press
    /// matters: without it there is no way back to whatever order the data
    /// was in to begin with.
    pub fn toggle_sort(&mut self, key: &SharedString) {
        self.sort = match self.order_of(key) {
            None => Some((key.clone(), SortOrder::Ascending)),
            Some(SortOrder::Ascending) => Some((key.clone(), SortOrder::Descending)),
            Some(SortOrder::Descending) => None,
        };
    }

    /// How wide a column is now.
    pub fn width_of(&self, column: &Column) -> f32 {
        self.widths
            .get(&column.key)
            .copied()
            .unwrap_or(column.width)
            .max(MIN_COLUMN)
    }

    pub fn set_width(&mut self, key: impl Into<SharedString>, width: f32) {
        self.widths.insert(key.into(), width.max(MIN_COLUMN));
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn select(&mut self, row: usize) {
        self.selected = Some(row);
    }

    pub fn clear_selection(&mut self) {
        self.selected = None;
    }

    pub fn is_resizing(&self) -> bool {
        self.grab.is_some()
    }

    /// Note where a resize started.
    fn begin_resize(&mut self, column: &Column, pointer: f32) {
        self.grab = Some((column.key.clone(), pointer, self.width_of(column)));
    }

    /// Follow the pointer. Returns whether anything changed.
    fn resize_to(&mut self, pointer: f32) -> bool {
        let Some((key, started_at, started_width)) = self.grab.clone() else {
            return false;
        };
        let width = (started_width + (pointer - started_at)).max(MIN_COLUMN);
        if (self.widths.get(&key).copied().unwrap_or(started_width) - width).abs() < 0.01
            && self.widths.contains_key(&key)
        {
            return false;
        }
        self.widths.insert(key, width);
        true
    }

    fn end_resize(&mut self) {
        self.grab = None;
    }
}

/// A table, drawn.
///
/// `cell` is asked for the contents of one cell at a time, so the data stays
/// where it is and cells can hold anything rather than only text.
pub fn table(
    columns: &[Column],
    rows: usize,
    state: Model<TableState>,
    cell: impl Fn(usize, &Column) -> Div + 'static,
) -> Div {
    let cell = Rc::new(cell);
    let mut grid = div()
        .class("table")
        .col()
        .role(crate::access::AccessRole::Table);

    // -- the header --------------------------------------------------------
    let mut header = div()
        .class("table-header")
        .row()
        .role(crate::access::AccessRole::Row);

    for col in columns {
        let width = Px(state.read().width_of(col));
        let order = state.read().order_of(col.key());

        let mut head = div()
            .class("table-head")
            .row()
            .items_center()
            // The grab handle on the trailing edge is placed against this,
            // so it has to be something to be placed against.
            .relative()
            .w(width)
            // A header keeps the width it was given whatever the cells under
            // it would rather be, or the columns stop lining up.
            .shrink(0.0)
            .overflow_hidden()
            .role(crate::access::AccessRole::ColumnHeader)
            .label(col.title().clone())
            .child(text(col.title().clone()).whitespace_nowrap());

        if let Some(order) = order {
            head = head.child(text(order.arrow()).class("table-sort").decorative());
        }

        if col.sortable {
            let sort = state.clone();
            let key = col.key().clone();
            let resizable = col.resizable;
            head = head.cursor_pointer().on_click(move |event, cx| {
                // A press on the trailing edge was a resize. Stopping the
                // press from propagating does not stop the click that
                // follows it, so the heading has to recognise the gesture
                // itself, or every column dragged wider also gets sorted.
                if resizable {
                    let right = event.bounds.origin.x + event.bounds.size.width;
                    if right - event.position.x <= Px(EDGE_HIT) {
                        return;
                    }
                }
                sort.update(|state| state.toggle_sort(&key));
                cx.request_redraw();
            });
        }

        if col.resizable {
            head = head.child(resize_edge(col, state.clone()));
        }
        header = header.child(head);
    }
    grid = grid.child(header);

    // -- the rows ----------------------------------------------------------
    let selected = state.read().selected();
    let mut body = div().class("table-body").col();

    for row in 0..rows {
        let mut line = div()
            .class("table-row")
            .row()
            .state_if(StateFlags::SELECTED, selected == Some(row))
            .role(crate::access::AccessRole::Row)
            .cursor_pointer();

        for col in columns {
            let width = Px(state.read().width_of(col));
            line = line.child(
                div()
                    .class("table-cell")
                    .w(width)
                    .shrink(0.0)
                    .overflow_hidden()
                    .role(crate::access::AccessRole::Cell)
                    .child(cell(row, col)),
            );
        }

        let choose = state.clone();
        line = line.on_click(move |_, cx| {
            choose.update(|state| state.select(row));
            cx.request_redraw();
        });
        body = body.child(line);
    }

    grid.child(body)
}

/// The grab area on a column's trailing edge.
fn resize_edge(col: &Column, state: Model<TableState>) -> Div {
    let column = col.clone();
    let press = state.clone();
    let drag = state.clone();
    let release = state;

    div()
        .class("table-resize")
        .absolute()
        .right(Px(0.0))
        .top(Px(0.0))
        .bottom(Px(0.0))
        .w(Px(EDGE_HIT))
        .shrink(0.0)
        .cursor(CursorStyle::ColResize)
        .on_mouse_down(move |event, cx| {
            press.update(|state| state.begin_resize(&column, event.position.x.0));
            // Or the header underneath would also take the press as a request
            // to sort, and every resize would reorder the table.
            cx.stop_propagation();
            cx.request_redraw();
        })
        .on_mouse_move(move |event, cx| {
            if !cx.input.is_down(crate::event::MouseButton::Left) {
                return;
            }
            if drag.update(|state| state.resize_to(event.position.x.0)) {
                cx.request_redraw();
            }
        })
        .on_mouse_up(move |_, cx| {
            if release.read().is_resizing() {
                release.update(|state| state.end_resize());
                cx.request_redraw();
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns() -> Vec<Column> {
        vec![
            table_column("name", "Name").width(200.0),
            table_column("size", "Size").width(90.0),
            table_column("kind", "Kind").sortable(false),
        ]
    }

    #[test]
    fn sorting_goes_up_then_down_then_away() {
        // The third press is the one that matters: without it there is no way
        // back to the order the data arrived in.
        let mut state = TableState::default();
        let name = SharedString::from("name");

        state.toggle_sort(&name);
        assert_eq!(state.order_of(&name), Some(SortOrder::Ascending));
        state.toggle_sort(&name);
        assert_eq!(state.order_of(&name), Some(SortOrder::Descending));
        state.toggle_sort(&name);
        assert_eq!(state.order_of(&name), None);
        assert_eq!(state.sort(), None);
    }

    #[test]
    fn sorting_by_another_column_starts_it_over() {
        let mut state = TableState::default();
        let name = SharedString::from("name");
        let size = SharedString::from("size");

        state.toggle_sort(&name);
        state.toggle_sort(&name);
        assert_eq!(state.order_of(&name), Some(SortOrder::Descending));

        // A different column starts ascending rather than inheriting the
        // direction the previous one happened to be left in.
        state.toggle_sort(&size);
        assert_eq!(state.order_of(&size), Some(SortOrder::Ascending));
        assert_eq!(state.order_of(&name), None, "two columns claimed the sort");
    }

    #[test]
    fn a_column_keeps_the_width_it_was_declared_with() {
        let state = TableState::default();
        let columns = columns();
        assert_eq!(state.width_of(&columns[0]), 200.0);
        assert_eq!(state.width_of(&columns[1]), 90.0);
    }

    #[test]
    fn dragging_an_edge_widens_by_how_far_the_pointer_moved() {
        let mut state = TableState::default();
        let columns = columns();
        state.begin_resize(&columns[0], 500.0);
        assert!(state.resize_to(560.0));
        assert_eq!(state.width_of(&columns[0]), 260.0);
    }

    #[test]
    fn a_column_cannot_be_dragged_shut() {
        // The handle that would widen it again goes with it, so a column of
        // no width cannot be recovered.
        let mut state = TableState::default();
        let columns = columns();
        state.begin_resize(&columns[1], 500.0);
        state.resize_to(0.0);
        assert_eq!(state.width_of(&columns[1]), MIN_COLUMN);
    }

    #[test]
    fn resizing_back_and_forth_does_not_drift() {
        let mut state = TableState::default();
        let columns = columns();
        state.begin_resize(&columns[0], 500.0);
        for x in [520.0, 580.0, 640.0, 480.0, 500.0] {
            state.resize_to(x);
        }
        assert_eq!(state.width_of(&columns[0]), 200.0);
    }

    #[test]
    fn a_width_survives_the_drag_that_set_it() {
        let mut state = TableState::default();
        let columns = columns();
        state.begin_resize(&columns[0], 100.0);
        state.resize_to(150.0);
        state.end_resize();
        assert!(!state.is_resizing());
        assert_eq!(state.width_of(&columns[0]), 250.0);
    }

    #[test]
    fn moving_with_nothing_grabbed_changes_nothing() {
        let mut state = TableState::default();
        assert!(!state.resize_to(900.0));
    }

    #[test]
    fn a_declared_width_below_the_minimum_is_raised_to_it() {
        let narrow = table_column("x", "X").width(1.0);
        assert_eq!(narrow.default_width(), MIN_COLUMN);
    }

    #[test]
    fn a_table_can_start_out_sorted() {
        let state = TableState::sorted_by("size", SortOrder::Descending);
        assert_eq!(
            state.order_of(&SharedString::from("size")),
            Some(SortOrder::Descending)
        );
        // And the next press carries on from there rather than starting over.
        let mut state = state;
        state.toggle_sort(&SharedString::from("size"));
        assert_eq!(state.sort(), None);
    }

    #[test]
    fn choosing_a_row_replaces_whatever_was_chosen() {
        let mut state = TableState::default();
        assert_eq!(state.selected(), None);
        state.select(3);
        state.select(7);
        assert_eq!(state.selected(), Some(7));
        state.clear_selection();
        assert_eq!(state.selected(), None);
    }
}
