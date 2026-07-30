pub mod applications;
pub mod connections;
pub mod dashboard;
pub mod settings;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use relm4::gtk;
use relm4::gtk::prelude::*;

use crate::state::cmp_rows;

/// Mutable sort key attached to a row; the section's sort function
/// reads the cells, so updating traffic re-sorts live.
pub struct SortKey {
    pub primary: Cell<u64>,
    pub secondary: Cell<u64>,
    pub name: String,
}

/// A titled section with its own boxed ListBox, live-sorted by byte
/// counters. Rows are mapped to their SortKey by widget pointer — safe
/// and explicit (entries are removed together with their rows).
pub struct SectionList {
    /// Heading + list; hide this to collapse the whole section.
    pub root: gtk::Box,
    pub list: gtk::ListBox,
    keys: Rc<RefCell<HashMap<usize, Rc<SortKey>>>>,
}

impl SectionList {
    pub fn new(title: &str) -> Self {
        let heading = gtk::Label::new(Some(title));
        heading.add_css_class("heading");
        heading.set_halign(gtk::Align::Start);

        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.append(&heading);
        root.append(&list);

        let keys: Rc<RefCell<HashMap<usize, Rc<SortKey>>>> = Rc::new(RefCell::new(HashMap::new()));
        {
            let keys = keys.clone();
            list.set_sort_func(move |a, b| {
                let map = keys.borrow();
                let lookup = |row: &gtk::ListBoxRow| map.get(&(row.as_ptr() as usize));
                match (lookup(a), lookup(b)) {
                    (Some(x), Some(y)) => to_gtk_ordering(cmp_rows(
                        (x.primary.get(), x.secondary.get(), &x.name),
                        (y.primary.get(), y.secondary.get(), &y.name),
                    )),
                    // Unknown rows keep their relative order.
                    _ => gtk::Ordering::Equal,
                }
            });
        }

        Self { root, list, keys }
    }

    /// Add a row (or re-add it after removal, e.g. on a group move).
    pub fn insert(&self, row: &gtk::ListBoxRow, key: Rc<SortKey>) {
        self.keys
            .borrow_mut()
            .insert(row.as_ptr() as usize, key);
        self.list.append(row);
        self.list.invalidate_sort();
    }

    pub fn remove(&self, row: &gtk::ListBoxRow) {
        self.keys.borrow_mut().remove(&(row.as_ptr() as usize));
        self.list.remove(row);
    }

    pub fn len(&self) -> usize {
        self.keys.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Re-apply the sort function after key cells changed.
    pub fn invalidate(&self) {
        self.list.invalidate_sort();
    }
}

fn to_gtk_ordering(ord: std::cmp::Ordering) -> gtk::Ordering {
    match ord {
        std::cmp::Ordering::Less => gtk::Ordering::Smaller,
        std::cmp::Ordering::Equal => gtk::Ordering::Equal,
        std::cmp::Ordering::Greater => gtk::Ordering::Larger,
    }
}
