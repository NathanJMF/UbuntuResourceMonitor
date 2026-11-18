use std::time::Duration;

use adw::{prelude::*, subclass::prelude::*};
use gtk::FlowBoxChild;
use gtk::glib::{self, DateTime, Priority, clone};
use log::trace;

use crate::config::PROFILE;
use crate::i18n::{i18n, i18n_f};
use crate::ui::widgets::graph_box::ResGraphBox;
use crate::utils::cpu::{CpuData, CpuInfo, physical_core_threads};
use crate::utils::settings::SETTINGS;
use crate::utils::units::{convert_frequency, convert_temperature, format_time_integer};
use crate::utils::{FiniteOr, NUM_CPUS, boot_time};

use super::top_processes::{self, Metric, ProcessRow, Ranking, TopTable};

pub const TAB_ID: &str = "cpu";

struct CoreTile {
    graph: ResGraphBox,
    name: String,
    threads: Vec<usize>,
}

#[derive(Clone, Copy, Default)]
enum UsageMode {
    #[default]
    Total,
    Physical,
    Logical,
}

mod imp {
    use std::cell::{Cell, RefCell};

    use crate::ui::{pages::CPU_PRIMARY_ORD, widgets::graph_box::ResGraphBox};

    use super::*;

    use gtk::{
        CompositeTemplate,
        gio::{Icon, ThemedIcon},
        glib::{ParamSpec, Properties, Value},
    };

    #[derive(CompositeTemplate, Properties)]
    #[template(resource = "/net/nokyan/Resources/ui/pages/cpu.ui")]
    #[properties(wrapper_type = super::ResCPU)]
    pub struct ResCPU {
        #[template_child]
        pub top_cpu: TemplateChild<adw::PreferencesGroup>,
        pub top_table: std::cell::OnceCell<TopTable>,
        pub(super) usage_mode: Cell<UsageMode>,
        #[template_child]
        pub usage_heading: TemplateChild<gtk::Button>,
        #[template_child]
        pub physical_page: TemplateChild<gtk::Box>,
        #[template_child]
        pub core_box: TemplateChild<gtk::FlowBox>,
        #[template_child]
        pub no_topology: TemplateChild<gtk::Label>,
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub total_page: TemplateChild<gtk::Box>,
        #[template_child]
        pub logical_page: TemplateChild<gtk::Box>,
        #[template_child]
        pub total_cpu: TemplateChild<ResGraphBox>,
        #[template_child]
        pub thread_box: TemplateChild<gtk::FlowBox>,
        #[template_child]
        pub max_speed: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub logical_cpus: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub physical_cpus: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub sockets: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub uptime: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub virtualization: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub architecture: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub temperature: TemplateChild<ResGraphBox>,
        pub thread_graphs: RefCell<Vec<ResGraphBox>>,
        pub(super) core_tiles: RefCell<Vec<CoreTile>>,
        pub old_total_usage: Cell<(u64, u64)>,
        pub old_thread_usages: RefCell<Vec<(u64, u64)>>,
        pub logical_cpus_amount: Cell<usize>,

        #[property(get)]
        uses_progress_bar: Cell<bool>,

        #[property(get)]
        main_graph_color: glib::Bytes,

        #[property(get)]
        icon: RefCell<Icon>,

        #[property(get, set)]
        usage: Cell<f64>,

        #[property(get = Self::tab_name, type = glib::GString)]
        tab_name: Cell<glib::GString>,

        #[property(get = Self::tab_detail_string, set = Self::set_tab_detail_string, type = glib::GString)]
        tab_detail_string: Cell<glib::GString>,

        #[property(get = Self::tab_usage_string, set = Self::set_tab_usage_string, type = glib::GString)]
        tab_usage_string: Cell<glib::GString>,

        #[property(get = Self::tab_id, type = glib::GString)]
        tab_id: Cell<glib::GString>,

        #[property(get)]
        graph_locked_max_y: Cell<bool>,

        #[property(get)]
        primary_ord: Cell<u32>,

        #[property(get)]
        secondary_ord: Cell<u32>,
    }

    impl ResCPU {
        gstring_getter_setter!(tab_name, tab_detail_string, tab_usage_string, tab_id);
    }

    impl Default for ResCPU {
        fn default() -> Self {
            Self {
                top_cpu: Default::default(),
                top_table: Default::default(),
                usage_mode: Default::default(),
                usage_heading: Default::default(),
                physical_page: Default::default(),
                core_box: Default::default(),
                no_topology: Default::default(),
                stack: Default::default(),
                total_page: Default::default(),
                logical_page: Default::default(),
                total_cpu: Default::default(),
                thread_box: Default::default(),
                max_speed: Default::default(),
                logical_cpus: Default::default(),
                physical_cpus: Default::default(),
                sockets: Default::default(),
                uptime: Default::default(),
                virtualization: Default::default(),
                architecture: Default::default(),
                temperature: Default::default(),
                thread_graphs: Default::default(),
                core_tiles: Default::default(),
                uses_progress_bar: Cell::new(true),
                main_graph_color: glib::Bytes::from_static(&super::ResCPU::MAIN_GRAPH_COLOR),
                icon: RefCell::new(ThemedIcon::new("processor-symbolic").into()),
                usage: Default::default(),
                tab_name: Cell::new(glib::GString::from(i18n("Processor"))),
                tab_detail_string: Cell::new(glib::GString::new()),
                tab_usage_string: Cell::new(glib::GString::new()),
                tab_id: Cell::new(glib::GString::from(TAB_ID)),
                old_total_usage: Cell::default(),
                old_thread_usages: RefCell::default(),
                logical_cpus_amount: Cell::default(),
                graph_locked_max_y: Cell::new(true),
                primary_ord: Cell::new(CPU_PRIMARY_ORD),
                secondary_ord: Default::default(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ResCPU {
        const NAME: &'static str = "ResCPU";
        type Type = super::ResCPU;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);
        }

        // You must call `Widget`'s `init_template()` within `instance_init()`.
        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ResCPU {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            // Devel Profile
            if PROFILE == "Devel" {
                obj.add_css_class("devel");
            }
        }

        fn properties() -> &'static [ParamSpec] {
            Self::derived_properties()
        }

        fn set_property(&self, id: usize, value: &Value, pspec: &ParamSpec) {
            self.derived_set_property(id, value, pspec);
        }

        fn property(&self, id: usize, pspec: &ParamSpec) -> Value {
            self.derived_property(id, pspec)
        }
    }

    impl WidgetImpl for ResCPU {}
    impl BinImpl for ResCPU {}
}

glib::wrapper! {
    pub struct ResCPU(ObjectSubclass<imp::ResCPU>)
        @extends gtk::Widget, adw::Bin,
        @implements gtk::Buildable, gtk::ConstraintTarget, gtk::Accessible;
}

impl Default for ResCPU {
    fn default() -> Self {
        Self::new()
    }
}

impl ResCPU {
    const MAIN_GRAPH_COLOR: [u8; 3] = [0x35, 0x84, 0xe4];

    pub fn new() -> Self {
        trace!("Creating ResCPU GObject…");

        glib::Object::new::<Self>()
    }

    pub fn init(&self, cpu_info: CpuInfo) {
        let imp = self.imp();
        imp.top_table.get_or_init(|| {
            TopTable::new(
                &imp.top_cpu,
                vec![Metric {
                    title: "CPU usage",
                    format: top_processes::percentage,
                }],
                vec![Ranking {
                    title: "CPU usage",
                    value: top_processes::first,
                }],
            )
        });
        self.setup_widgets(cpu_info);
        self.setup_signals();
    }

    pub fn setup_widgets(&self, cpu_info: CpuInfo) {
        trace!("Setting up ResCPU widgets…");

        let imp = self.imp();

        let logical_cpus = cpu_info.logical_cpus.unwrap_or(0);

        let CpuData {
            new_thread_usages,
            temperature: _,
            frequencies: _,
        } = CpuData::new(logical_cpus);

        let logical_cpus = logical_cpus.max(new_thread_usages.len());
        let old_total_usage = new_thread_usages
            .iter()
            .flatten()
            .copied()
            .reduce(|acc, x| (acc.0 + x.0, acc.1 + x.1))
            .unwrap_or_default();
        imp.old_total_usage.set(old_total_usage);

        for i in 0..logical_cpus {
            let old_thread_usage = new_thread_usages
                .get(i)
                .map_or((0, 0), |i| *i.as_ref().unwrap_or(&(0, 0)));
            imp.old_thread_usages.borrow_mut().push(old_thread_usage);
        }

        imp.logical_cpus_amount.set(logical_cpus);

        imp.total_cpu.set_title_label(&i18n("Total Usage"));
        imp.total_cpu.set_subtitle_selectable(false);
        imp.total_cpu.set_subtitle(&i18n("N/A"));
        imp.total_cpu.graph().set_graph_color(
            Self::MAIN_GRAPH_COLOR[0],
            Self::MAIN_GRAPH_COLOR[1],
            Self::MAIN_GRAPH_COLOR[2],
        );

        for i in 0..logical_cpus {
            let thread_box = ResGraphBox::new();
            thread_box.set_subtitle_selectable(false);
            thread_box.set_subtitle(&i18n_f("CPU {}", &[&(i + 1).to_string()]));
            thread_box.set_title_label(&i18n("N/A"));
            thread_box.add_css_class("compact-cpu");
            thread_box.set_width_request(96);
            thread_box.graph().set_height_request(40);
            thread_box.graph().set_graph_color(28, 113, 216);
            let flow_box_chld = FlowBoxChild::builder()
                .child(&thread_box)
                .can_target(false)
                .build();
            imp.thread_box.append(&flow_box_chld);
            imp.thread_graphs.borrow_mut().push(thread_box);
        }

        let core_threads = physical_core_threads(logical_cpus);
        imp.no_topology.set_visible(core_threads.is_empty());
        for (index, cpu_ids) in core_threads.into_iter().enumerate() {
            let graph = ResGraphBox::new();
            graph.set_subtitle_selectable(false);
            let name = i18n_f("Core {}", &[&(index + 1).to_string()]);
            graph.set_title_label(&name);
            graph.set_subtitle(&i18n("N/A"));
            graph.add_css_class("compact-cpu");
            graph.set_width_request(96);
            graph.graph().set_height_request(40);
            graph.graph().set_graph_color(28, 113, 216);

            imp.core_box.append(
                &FlowBoxChild::builder()
                    .child(&graph)
                    .can_target(false)
                    .build(),
            );
            imp.core_tiles.borrow_mut().push(CoreTile {
                graph,
                name,
                threads: cpu_ids,
            });
        }

        imp.temperature.set_title_label(&i18n("Temperature"));
        imp.temperature.graph().set_graph_color(0x1a, 0x5f, 0xb4);
        imp.temperature.graph().set_locked_max_y(None);

        imp.max_speed.set_subtitle(
            &cpu_info
                .max_speed
                .map_or_else(|| i18n("N/A"), convert_frequency),
        );

        imp.logical_cpus.set_subtitle(
            &cpu_info
                .logical_cpus
                .map_or_else(|| i18n("N/A"), |x| x.to_string()),
        );

        imp.physical_cpus.set_subtitle(
            &cpu_info
                .physical_cpus
                .map_or_else(|| i18n("N/A"), |x| x.to_string()),
        );

        imp.sockets.set_subtitle(
            &cpu_info
                .sockets
                .map_or_else(|| i18n("N/A"), |x| x.to_string()),
        );

        imp.virtualization
            .set_subtitle(&cpu_info.virtualization.unwrap_or_else(|| i18n("N/A")));

        imp.architecture
            .set_subtitle(&cpu_info.architecture.unwrap_or_else(|| i18n("N/A")));

        if let Some(model_name) = cpu_info.model_name {
            imp.set_tab_detail_string(&model_name);
        }
    }

    fn set_usage_mode(&self, mode: UsageMode) {
        let imp = self.imp();
        imp.usage_mode.set(mode);
        let label = match mode {
            UsageMode::Total => {
                imp.stack.set_visible_child(&imp.total_page.get());
                let _ = SETTINGS.set_show_logical_cpus(false);
                i18n("Total")
            }
            UsageMode::Physical => {
                imp.stack.set_visible_child(&imp.physical_page.get());
                i18n("Physical Cores")
            }
            UsageMode::Logical => {
                imp.stack.set_visible_child(&imp.logical_page.get());
                let _ = SETTINGS.set_show_logical_cpus(true);
                i18n("Logical Threads")
            }
        };
        imp.usage_heading
            .set_label(&i18n_f("Usage — {}", &[&label]));
    }

    fn cycle_usage_mode(&self) {
        let mode = match self.imp().usage_mode.get() {
            UsageMode::Total => UsageMode::Physical,
            UsageMode::Physical => UsageMode::Logical,
            UsageMode::Logical => UsageMode::Total,
        };
        self.set_usage_mode(mode);
    }

    pub fn setup_signals(&self) {
        trace!("Setting up ResCPU signals…");

        let imp = self.imp();
        imp.usage_heading.connect_clicked(clone!(
            #[weak(rename_to = this)]
            self,
            move |_| this.cycle_usage_mode()
        ));

        let usage_click = gtk::GestureClick::new();
        usage_click.set_button(1);
        usage_click.connect_released(clone!(
            #[weak(rename_to = this)]
            self,
            move |_, presses, _, _| {
                if presses == 1 {
                    this.cycle_usage_mode();
                }
            }
        ));
        imp.stack.set_cursor_from_name(Some("pointer"));
        imp.stack.add_controller(usage_click);

        glib::timeout_add_local_full(
            Duration::from_secs(1),
            Priority::DEFAULT,
            clone!(
                #[strong(rename_to = this)]
                self,
                move || {
                    let imp = this.imp();

                    if let (Ok(boot_time), Ok(now)) = (boot_time(), DateTime::now_local()) {
                        let time_difference = now.difference(&boot_time).as_seconds() as isize;
                        imp.uptime
                            .set_subtitle(&format_time_integer(time_difference));
                    } else {
                        imp.uptime.set_subtitle(&i18n("N/A"));
                    }

                    glib::ControlFlow::Continue
                }
            ),
        );

        if SETTINGS.show_logical_cpus() {
            self.set_usage_mode(UsageMode::Logical);
        }
    }

    pub(in crate::ui) fn refresh_top_processes(&self, processes: &crate::utils::app::AppsContext) {
        let rows = processes
            .processes_iter()
            .map(|process| {
                let values = {
                    let ratio = f64::from(process.cpu_time_ratio());
                    let ratio = if SETTINGS.normalize_cpu_usage() {
                        ratio
                    } else {
                        ratio * *NUM_CPUS as f64
                    };
                    vec![Some(ratio)]
                };
                ProcessRow {
                    pid: process.data.pid,
                    starttime: process.data.starttime,
                    name: process.display_name.clone(),
                    values,
                }
            })
            .collect();
        if let Some(table) = self.imp().top_table.get() {
            table.set_rows(rows);
        }
    }

    pub fn refresh_page(&self, cpu_data: &CpuData) {
        trace!("Refreshing ResCPU…");

        let CpuData {
            new_thread_usages,
            temperature,
            frequencies,
        } = cpu_data;

        let imp = self.imp();

        let new_total_usage = new_thread_usages
            .iter()
            .flatten()
            .copied()
            .reduce(|acc, x| (acc.0 + x.0, acc.1 + x.1))
            .unwrap_or_default();

        let idle_total_delta = new_total_usage
            .0
            .saturating_sub(imp.old_total_usage.get().0);
        let sum_total_delta = new_total_usage
            .1
            .saturating_sub(imp.old_total_usage.get().1);
        let work_total_time = sum_total_delta.saturating_sub(idle_total_delta);

        let total_fraction =
            ((work_total_time as f64) / (sum_total_delta as f64)).finite_or_default();

        imp.total_cpu.graph().push_data_point(total_fraction);

        let mut percentage = total_fraction * 100.0;
        if !SETTINGS.normalize_cpu_usage() {
            percentage *= *NUM_CPUS as f64;
        }

        let mut percentage_string = format!("{} %", percentage.round());
        imp.total_cpu.set_subtitle(&percentage_string);

        imp.old_total_usage.set(new_total_usage);

        let mut thread_deltas = vec![(0, 0); imp.logical_cpus_amount.get()];
        for (i, old_thread_usage) in imp.old_thread_usages.borrow_mut().iter_mut().enumerate() {
            let thread_graphs = imp.thread_graphs.borrow();
            let curr_threadbox = &thread_graphs[i];
            let Some(Ok(new_thread_usage)) = new_thread_usages.get(i) else {
                curr_threadbox.set_subtitle(&i18n("N/A"));
                continue;
            };
            let idle = new_thread_usage.0.saturating_sub(old_thread_usage.0);
            let total = new_thread_usage.1.saturating_sub(old_thread_usage.1);
            let active = total.saturating_sub(idle);
            thread_deltas[i] = (active, total);
            let fraction = (active as f64 / total as f64).finite_or_default();

            curr_threadbox.graph().push_data_point(fraction);
            curr_threadbox.set_subtitle(&format!("{} %", (fraction * 100.0).round()));

            let name = i18n_f("CPU {}", &[&(i + 1).to_string()]);
            if let Some(frequency) = frequencies.get(i).copied().flatten() {
                curr_threadbox
                    .set_title_label(&format!("{name} · {}", convert_frequency(frequency as f64)));
            } else {
                curr_threadbox.set_title_label(&name);
            }
            *old_thread_usage = *new_thread_usage;
        }

        for core in imp.core_tiles.borrow().iter() {
            let frequency = core
                .threads
                .iter()
                .filter_map(|&id| frequencies.get(id).copied().flatten())
                .max()
                .map_or_else(|| i18n("N/A"), |hz| convert_frequency(hz as f64));
            core.graph
                .set_title_label(&format!("{} · {frequency}", core.name));

            let mut active = 0;
            let mut total = 0;
            for &id in &core.threads {
                let (thread_active, thread_total) = thread_deltas[id];
                active += thread_active;
                total += thread_total;
            }
            if total > 0 {
                let fraction = active as f64 / total as f64;
                core.graph.graph().push_data_point(fraction);
                core.graph
                    .set_subtitle(&format!("{:.0} %", 100.0 * fraction));
            } else {
                core.graph.set_subtitle(&i18n("N/A"));
            }
        }

        imp.temperature.graph().set_visible(temperature.is_ok());

        if let Ok(temperature) = temperature {
            let temperature_string = convert_temperature(f64::from(*temperature));

            let highest_temperature_string =
                convert_temperature(imp.temperature.graph().get_highest_value());

            imp.temperature.set_subtitle(&format!(
                "{} · {} {}",
                &temperature_string,
                i18n("Highest:"),
                highest_temperature_string
            ));
            imp.temperature
                .graph()
                .push_data_point(f64::from(*temperature));

            percentage_string.push_str(" · ");
            percentage_string.push_str(&temperature_string);
        } else {
            imp.temperature.set_subtitle(&i18n("N/A"));
        }

        self.set_property("usage", total_fraction);

        self.set_property("tab_usage_string", percentage_string);
    }
}
