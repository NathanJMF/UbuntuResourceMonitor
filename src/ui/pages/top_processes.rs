use std::{cell::RefCell, rc::Rc};

use adw::prelude::*;
use gtk::{gio, glib};

use crate::{i18n::i18n, ui::window::MainWindow};

const PROCESS_LIMIT: usize = 10;

pub struct ProcessRow {
    pub pid: i32,
    pub starttime: u64,
    pub name: String,
    pub values: Vec<Option<f64>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ProcessIdentity {
    pid: i32,
    starttime: u64,
}

struct TableItem {
    process: Option<ProcessIdentity>,
    cells: Vec<String>,
}

fn selected_process(selection: &gtk::SingleSelection) -> Option<ProcessIdentity> {
    selection
        .selected_item()
        .and_downcast::<glib::BoxedAnyObject>()?
        .borrow::<TableItem>()
        .process
}

pub struct Metric {
    pub title: &'static str,
    pub format: fn(f64) -> String,
}

pub struct Ranking {
    pub title: &'static str,
    pub value: fn(&ProcessRow) -> f64,
}

pub struct TopTable {
    rows: Rc<RefCell<Vec<ProcessRow>>>,
    store: gio::ListStore,
    selection: gtk::SingleSelection,
    ranking: Option<gtk::DropDown>,
    metrics: Rc<Vec<Metric>>,
    rankings: Rc<Vec<Ranking>>,
}

impl TopTable {
    pub fn new(
        group: &adw::PreferencesGroup,
        metrics: Vec<Metric>,
        rankings: Vec<Ranking>,
    ) -> Self {
        let title = group.title();
        let notes = group.description();
        group.set_title("");
        group.set_description(None);
        group.update_property(&[gtk::accessible::Property::Label(&title)]);
        let container = gtk::Box::new(gtk::Orientation::Vertical, 8);
        group.add(&container);
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = gtk::SingleSelection::new(Some(store.clone()));
        selection.set_autoselect(false);
        selection.set_can_unselect(true);
        let table = gtk::ColumnView::new(Some(selection.clone()));
        table.set_show_row_separators(true);
        let headers = [i18n("Process"), i18n("PID")]
            .into_iter()
            .chain(metrics.iter().map(|metric| i18n(metric.title)));
        let last_column = metrics.len() + 1;
        for (index, title) in headers.enumerate() {
            let factory = gtk::SignalListItemFactory::new();
            factory.connect_setup(move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().unwrap();
                let label = gtk::Label::new(None);
                label.set_xalign(if index == 0 { 0.0 } else { 1.0 });
                if index == 0 {
                    label.set_margin_start(10);
                } else if index == last_column {
                    label.set_margin_end(10);
                }
                label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                item.set_child(Some(&label));
            });
            factory.connect_bind(move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().unwrap();
                let label = item.child().and_downcast::<gtk::Label>().unwrap();
                let row = item.item().and_downcast::<glib::BoxedAnyObject>().unwrap();
                label.set_label(&row.borrow::<TableItem>().cells[index]);
            });
            let column = gtk::ColumnViewColumn::new(Some(&title), Some(factory));
            column.set_expand(index == 0);
            table.append_column(&column);
        }

        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let heading = gtk::Label::new(Some(&title));
        heading.add_css_class("heading");
        heading.set_ellipsize(gtk::pango::EllipsizeMode::End);
        heading.set_xalign(0.0);
        heading.set_tooltip_text(notes.as_deref());
        if let Some(notes) = &notes {
            heading.update_property(&[gtk::accessible::Property::Description(notes)]);
        }
        controls.append(&heading);
        let ranking = if rankings.len() > 1 {
            let titles: Vec<_> = rankings.iter().map(|ranking| i18n(ranking.title)).collect();
            let titles: Vec<_> = titles.iter().map(String::as_str).collect();
            let selector = gtk::DropDown::from_strings(&titles);
            selector.set_halign(gtk::Align::Start);
            let rank_by = i18n("Rank by");
            selector.update_property(&[gtk::accessible::Property::Label(&rank_by)]);
            controls.append(&selector);
            Some(selector)
        } else {
            None
        };

        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        controls.append(&spacer);
        let stop = gtk::Button::with_label(&i18n("Stop Process"));
        stop.set_sensitive(false);
        stop.add_css_class("destructive-action");
        controls.append(&stop);
        selection.connect_selected_item_notify(glib::clone!(
            #[weak]
            stop,
            move |selection| {
                stop.set_sensitive(selected_process(selection).is_some_and(|process| {
                    process.pid > 0 && process.pid as u32 != std::process::id()
                }));
            }
        ));
        stop.connect_clicked(glib::clone!(
            #[weak]
            selection,
            move |button| {
                if let Some(process) = selected_process(&selection) {
                    if let Some(window) = button.root().and_downcast::<MainWindow>() {
                        window.stop_ranked_process(process.pid, process.starttime);
                    }
                }
            }
        ));
        container.append(&controls);
        let table_card = gtk::Box::new(gtk::Orientation::Vertical, 0);
        table_card.add_css_class("card");
        table_card.add_css_class("resource-process-table");
        table_card.set_overflow(gtk::Overflow::Hidden);
        table_card.append(&table);
        container.append(&table_card);
        let result = Self {
            rows: Rc::default(),
            store,
            selection,
            ranking,
            metrics: Rc::new(metrics),
            rankings: Rc::new(rankings),
        };
        if let Some(selector) = &result.ranking {
            let rows = Rc::clone(&result.rows);
            let metrics = Rc::clone(&result.metrics);
            let rankings = Rc::clone(&result.rankings);
            let store = result.store.clone();
            let selection = result.selection.clone();
            selector.connect_selected_notify(move |selector| {
                Self::render(
                    &store,
                    &selection,
                    &rows.borrow(),
                    &metrics,
                    &rankings,
                    selector.selected(),
                );
            });
        }
        result.set_rows(Vec::new());
        result
    }

    pub fn set_rows(&self, rows: Vec<ProcessRow>) {
        *self.rows.borrow_mut() = rows;
        Self::render(
            &self.store,
            &self.selection,
            &self.rows.borrow(),
            &self.metrics,
            &self.rankings,
            self.ranking.as_ref().map_or(0, gtk::DropDown::selected),
        );
    }

    fn render(
        store: &gio::ListStore,
        selection: &gtk::SingleSelection,
        rows: &[ProcessRow],
        metrics: &[Metric],
        rankings: &[Ranking],
        index: u32,
    ) {
        let rank = rankings.get(index as usize).unwrap_or(&rankings[0]).value;
        let mut sorted: Vec<_> = rows
            .iter()
            .filter_map(|row| {
                let value = rank(row);
                (value.is_finite() && value > 0.0).then_some((row, value))
            })
            .collect();
        sorted.sort_unstable_by(|(left, a), (right, b)| {
            b.total_cmp(a).then_with(|| left.pid.cmp(&right.pid))
        });

        let mut items: Vec<_> = sorted
            .into_iter()
            .take(PROCESS_LIMIT)
            .map(|(row, _)| {
                let mut cells = vec![row.name.clone(), row.pid.to_string()];
                cells.extend(metrics.iter().zip(&row.values).map(|(metric, value)| {
                    value
                        .filter(|value| value.is_finite())
                        .map_or_else(|| "—".into(), metric.format)
                }));
                glib::BoxedAnyObject::new(TableItem {
                    process: Some(ProcessIdentity {
                        pid: row.pid,
                        starttime: row.starttime,
                    }),
                    cells,
                })
            })
            .collect();
        if items.is_empty() {
            let mut cells = vec![String::new(); metrics.len() + 2];
            cells[0] = i18n("No process activity reported");
            items.push(glib::BoxedAnyObject::new(TableItem {
                process: None,
                cells,
            }));
        }

        let previously_selected = selected_process(selection);
        let next_selection = previously_selected.and_then(|identity| {
            items
                .iter()
                .position(|item| item.borrow::<TableItem>().process == Some(identity))
        });
        store.splice(0, store.n_items(), &items);
        selection
            .set_selected(next_selection.map_or(gtk::INVALID_LIST_POSITION, |index| index as u32));
    }
}

pub fn percentage(value: f64) -> String {
    format!("{:.1} %", value * 100.0)
}

pub fn storage(value: f64) -> String {
    crate::utils::units::convert_storage(value, false)
}

pub fn first(row: &ProcessRow) -> f64 {
    row.values[0]
        .filter(|value| value.is_finite())
        .unwrap_or_default()
}

pub fn second(row: &ProcessRow) -> f64 {
    row.values[1]
        .filter(|value| value.is_finite())
        .unwrap_or_default()
}

pub fn combined(row: &ProcessRow) -> f64 {
    first(row) + second(row)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a GTK display"]
    fn changing_rank_preserves_columns_and_reorders_immediately() {
        gtk::init().unwrap();
        let group = adw::PreferencesGroup::new();
        let table = TopTable::new(
            &group,
            vec![
                Metric {
                    title: "GPU activity",
                    format: percentage,
                },
                Metric {
                    title: "Video memory",
                    format: storage,
                },
            ],
            vec![
                Ranking {
                    title: "GPU activity",
                    value: first,
                },
                Ranking {
                    title: "Video memory",
                    value: second,
                },
            ],
        );
        table.set_rows(vec![
            ProcessRow {
                pid: 1,
                starttime: 100,
                name: "Activity leader".into(),
                values: vec![Some(0.8), Some(512.0)],
            },
            ProcessRow {
                pid: 2,
                starttime: 200,
                name: "Memory leader".into(),
                values: vec![Some(0.2), Some(2048.0)],
            },
        ]);
        let first_row = || {
            let row = table
                .store
                .item(0)
                .and_downcast::<glib::BoxedAnyObject>()
                .unwrap();
            let cells = row.borrow::<TableItem>().cells.clone();
            cells
        };
        assert_eq!(first_row()[0], "Activity leader");
        table.selection.set_selected(0);
        table.ranking.as_ref().unwrap().set_selected(1);
        assert_eq!(table.selection.selected(), 1);
        assert_eq!(selected_process(&table.selection).unwrap().starttime, 100);
        let ranked = first_row();
        assert_eq!(ranked[0], "Memory leader");
        assert_eq!(ranked[1], "2");
        assert_eq!(ranked[2], percentage(0.2));
        assert_eq!(ranked[3], storage(2048.0));
    }
}
