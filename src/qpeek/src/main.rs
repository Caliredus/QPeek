// Konsol penceresi açılmasın (release ve debug).
#![windows_subsystem = "windows"]

mod accounts;
mod activation;
mod backend;
mod i18n;
mod icon;
mod notify;
mod theme;
#[cfg(windows)]
mod win;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Timelike;
use slint::{CloseRequestResponse, Color, ComponentHandle, Model, ModelRc, SharedString, VecModel};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use backend::Card;
use i18n::Lang;
use theme::ThemeId;

slint::include_modules!();

/// Çekirdeği kaç dakikada bir yoklayacağımız. Sağlayıcı uç noktalarını yormamak için seyrek.
const REFRESH_EVERY: Duration = Duration::from_secs(5 * 60);
/// Panel açılırken veri bundan eskiyse hemen yenile (CallidusQuota: 30 sn).
const STALE_ON_OPEN: Duration = Duration::from_secs(30);
/// Bu yaştan sonra durum etiketi "Eski" olur (CallidusQuota: 180 sn).
const OLD_AFTER: Duration = Duration::from_secs(180);
/// Yalnız pencerelerden biri görünürken sayaç ve odak kaybı için yoklama aralığı.
const VISIBLE_TICK: Duration = Duration::from_millis(100);
/// Açıldıktan hemen sonra odak kaybını yok say (CallidusQuota: 300 ms).
const OPEN_GRACE: Duration = Duration::from_millis(300);
/// Odak kaybıyla gizlenen pencereyi aynı tıklama yeniden açmasın.
const REOPEN_GUARD: Duration = Duration::from_millis(400);
/// Fare tekerleğiyle hesap değiştirirken hızlı ardışık olayları tek adımda tut.
const WHEEL_COOLDOWN: Duration = Duration::from_millis(150);
const MARGIN: f32 = 12.0;

thread_local! {
    static DRAIN: RefCell<Option<Rc<dyn Fn()>>> = RefCell::new(None);
    /// Yalnız tepsi paneli (MainWindow) görev çubuğundan gizlenir; AppWindow normal penceredir.
    static NEXT_SKIP_TASKBAR: Cell<bool> = const { Cell::new(false) };
}

enum AppEvent {
    ToggleTray,
    OpenApp,
    Menu(MenuId),
    Update(Update),
}

enum Update {
    Started,
    Done(Result<Vec<Card>, String>),
    CostDone(u32, Result<backend::CostSummary, String>),
    ActivationDone(String, String, Result<(), String>),
    /// Ayarlar: sağlayıcı durumları ve hesapları.
    SettingsData {
        providers: Result<Vec<ProviderData>, String>,
        action: Option<(String, Result<(), String>)>,
    },
}

#[derive(Clone)]
struct EventBus {
    tx: Arc<Mutex<Sender<AppEvent>>>,
}

impl EventBus {
    fn new(tx: Sender<AppEvent>) -> Self {
        Self { tx: Arc::new(Mutex::new(tx)) }
    }

    fn send(&self, ev: AppEvent) {
        if let Ok(tx) = self.tx.lock()
            && tx.send(ev).is_ok()
        {
            wake_event_loop();
        }
    }
}

fn wake_event_loop() {
    let _ = slint::invoke_from_event_loop(|| {
        let f = DRAIN.with(|d| d.borrow().as_ref().cloned());
        if let Some(f) = f {
            f();
        }
    });
}

#[derive(Clone)]
struct ProviderData {
    id: String,
    display_name: String,
    connected: bool,
    accounts: Vec<(String, Option<String>, bool)>, // (key, email, connected)
    can_add: bool,
    api_key_mode: i32, // 0 = none, 1 = required, 2 = optional
    api_env: String,
    api_help: String,
}

enum ProviderAction {
    Account { key: String, connect: bool },
    AddAccount,
    Toggle(bool),
    SaveApiKey(String),
}

#[derive(Clone, Copy)]
enum ProviderBusy {
    Working,
    SigningIn,
    SavingKey,
}

impl ProviderBusy {
    fn text(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Self::Working, Lang::Tr) => "İşleniyor…",
            (Self::Working, Lang::En) => "Working…",
            (Self::SigningIn, Lang::Tr) => "Tarayıcıda girişi tamamla… (en fazla 5 dk)",
            (Self::SigningIn, Lang::En) => "Complete sign-in in browser… (up to 5 min)",
            (Self::SavingKey, Lang::Tr) => "Anahtar kaydediliyor…",
            (Self::SavingKey, Lang::En) => "Saving key…",
        }
    }
}

#[derive(Clone)]
struct SettingsWorker {
    bus: EventBus,
    access: Arc<Mutex<()>>,
}

impl SettingsWorker {
    fn spawn(&self, action: Option<(String, ProviderAction)>) {
        let worker = self.clone();
        std::thread::spawn(move || {
            // Browser sign-in can take minutes. Other providers remain usable while it runs.
            let login = match &action {
                Some((id, ProviderAction::AddAccount)) => Some(accounts::login(id)),
                _ => None,
            };
            // Publish snapshots in the same order they are read, with config writes serialized.
            let _access = worker.access.lock().unwrap();
            let action = action.map(|(id, action)| {
                let result = match action {
                    ProviderAction::Account { key, connect } if key == accounts::DEFAULT_KEY => {
                        backend::set_enabled(&id, connect)
                    }
                    ProviderAction::Account { key, connect: false } => accounts::remove_extra(&id, &key),
                    ProviderAction::Account { .. } => Ok(()),
                    ProviderAction::AddAccount => login.unwrap(),
                    ProviderAction::Toggle(enabled) => backend::set_enabled(&id, enabled),
                    ProviderAction::SaveApiKey(key) => backend::set_api_key(&id, &key),
                };
                (id, result)
            });
            worker.bus.send(AppEvent::Update(Update::SettingsData {
                providers: load_settings_data(),
                action,
            }));
        });
    }
}

struct MenuItems {
    open_app: MenuItem,
    open_tray: MenuItem,
    refresh: MenuItem,
    about: MenuItem,
    quit: MenuItem,
}

impl MenuItems {
    fn set_lang(&self, lang: Lang) {
        self.open_app.set_text(i18n::menu_open_app(lang));
        self.open_tray.set_text(i18n::menu_open_tray(lang));
        self.refresh.set_text(i18n::menu_refresh(lang));
        self.about.set_text(i18n::menu_about(lang));
        self.quit.set_text(i18n::menu_quit(lang));
    }
}

#[derive(Clone)]
struct CardModels {
    tray: Rc<VecModel<ProviderCard>>,
    app: Rc<VecModel<ProviderCard>>,
}

#[derive(Clone)]
struct AppWorkers {
    refresh: Sender<()>,
    settings: SettingsWorker,
    cost: mpsc::Sender<Update>,
}

struct Prefs {
    lang: Lang,
    alerts_enabled: bool,
    theme: ThemeId,
}

impl Default for Prefs {
    fn default() -> Self {
        Self { lang: Lang::Tr, alerts_enabled: true, theme: ThemeId::Obsidian }
    }
}

struct Panel {
    cards: Vec<Card>,
    fetched_at: Option<Instant>,
    error: Option<String>,
    refreshing: bool,
    cost_days: u32,
    cost_data: Option<backend::CostSummary>,
    cost_loading: bool,
    cost_error: Option<String>,
    cost_pending: HashSet<u32>,
    shown_at: Option<Instant>,
    hidden_at: Option<Instant>,
    app_shown_at: Option<Instant>,
    app_hidden_at: Option<Instant>,
    last_wheel: Option<Instant>,
    peak: Option<f32>,
    autostart: bool,
    lang: Lang,
    alerts_enabled: bool,
    theme: ThemeId,
    alerts: notify::AlertTracker,
    activation: activation::ActivationConfig,
    act_busy: HashSet<String>,
    providers: Vec<ProviderData>,
    search_query: String,
    filter_mode: i32,
    selected_provider_id: String,
    busy: HashMap<String, ProviderBusy>,
    notes: HashMap<String, String>,
    selected: HashMap<String, String>,
    logos: RefCell<HashMap<String, Option<slint::Image>>>,
}

impl Default for Panel {
    fn default() -> Self {
        Self {
            cards: Vec::new(),
            fetched_at: None,
            error: None,
            refreshing: false,
            cost_days: 30,
            cost_data: None,
            cost_loading: false,
            cost_error: None,
            cost_pending: HashSet::new(),
            shown_at: None,
            hidden_at: None,
            app_shown_at: None,
            app_hidden_at: None,
            last_wheel: None,
            peak: None,
            autostart: false,
            lang: Lang::Tr,
            alerts_enabled: true,
            theme: ThemeId::Obsidian,
            alerts: notify::AlertTracker::default(),
            activation: activation::ActivationConfig::default(),
            act_busy: HashSet::new(),
            providers: Vec::new(),
            search_query: String::new(),
            filter_mode: 0,
            selected_provider_id: String::new(),
            busy: HashMap::new(),
            notes: HashMap::new(),
            selected: HashMap::new(),
            logos: RefCell::default(),
        }
    }
}

fn main() -> Result<(), slint::PlatformError> {
    #[cfg(windows)]
    win::set_app_id();

    let start_hidden = std::env::args().any(|a| a == "--tray");
    let (event_tx, event_rx) = mpsc::channel::<AppEvent>();
    let bus = EventBus::new(event_tx);

    #[cfg(windows)]
    {
        let bus = bus.clone();
        if let win::Instance::Secondary = win::single_instance(start_hidden, move || bus.send(AppEvent::OpenApp)) {
            return Ok(());
        }
    }

    // Global kısayol: Alt + Shift + Q -> tepsideki hızlı paneli aç/kapat.
    #[cfg(windows)]
    {
        let bus = bus.clone();
        win::spawn_hotkey(move || bus.send(AppEvent::ToggleTray));
    }

    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .with_winit_window_attributes_hook(|attrs| {
            #[cfg(windows)]
            {
                use slint::winit_030::winit::platform::windows::WindowAttributesExtWindows;
                attrs.with_skip_taskbar(NEXT_SKIP_TASKBAR.with(Cell::get))
            }
            #[cfg(not(windows))]
            attrs
        })
        .select()?;

    NEXT_SKIP_TASKBAR.with(|c| c.set(true));
    let tray_ui = MainWindow::new()?;
    NEXT_SKIP_TASKBAR.with(|c| c.set(false));
    let app_ui = AppWindow::new()?;
    tray_ui.global::<RefreshIcon>().on_frame(refresh_icon_frame);
    app_ui.global::<RefreshIcon>().on_frame(refresh_icon_frame);

    let card_models = CardModels {
        tray: Rc::new(VecModel::default()),
        app: Rc::new(VecModel::default()),
    };
    tray_ui.set_cards(ModelRc::from(card_models.tray.clone()));
    app_ui.set_cards(ModelRc::from(card_models.app.clone()));

    let (refresh_tx, refresh_rx) = mpsc::channel::<()>();
    spawn_refresher(refresh_rx, bus.clone());
    let settings_worker = SettingsWorker { bus: bus.clone(), access: Arc::new(Mutex::new(())) };
    let (cost_tx, cost_rx) = mpsc::channel();
    let app_workers = AppWorkers {
        refresh: refresh_tx.clone(),
        settings: settings_worker.clone(),
        cost: cost_tx.clone(),
    };

    #[cfg(windows)]
    let autostart = win::autostart_enabled();
    #[cfg(not(windows))]
    let autostart = false;

    let prefs = load_prefs();
    let debug_mode = if cfg!(debug_assertions) {
        std::env::var("QPEEK_DEBUG").ok()
    } else {
        None
    };
    let initial_lang = if debug_mode.as_deref() == Some("app-en") { Lang::En } else { prefs.lang };
    let mut initial_theme = prefs.theme;
    if cfg!(debug_assertions) && let Ok(code) = std::env::var("QPEEK_THEME") {
        initial_theme = ThemeId::from_code(&code);
    }
    let panel = Rc::new(RefCell::new(Panel {
        selected: load_selection(),
        autostart,
        lang: initial_lang,
        alerts_enabled: prefs.alerts_enabled,
        theme: initial_theme,
        activation: activation::load(),
        ..Panel::default()
    }));
    let prefs = Rc::new(RefCell::new(prefs));
    let about_window: Rc<RefCell<Option<AboutWindow>>> = Rc::new(RefCell::new(None));
    let visible_timer = Rc::new(slint::Timer::default());

    // Tepsi ikonu ve menüsü (olay güdümlü; dil değişince anında güncellenir).
    let menu_items = Rc::new(MenuItems {
        open_app: MenuItem::new(i18n::menu_open_app(initial_lang), true, None),
        open_tray: MenuItem::new(i18n::menu_open_tray(initial_lang), true, None),
        refresh: MenuItem::new(i18n::menu_refresh(initial_lang), true, None),
        about: MenuItem::new(i18n::menu_about(initial_lang), true, None),
        quit: MenuItem::new(i18n::menu_quit(initial_lang), true, None),
    });
    let menu = Menu::new();
    let _ = menu.append_items(&[
        &menu_items.open_app,
        &menu_items.open_tray,
        &menu_items.refresh,
        &PredefinedMenuItem::separator(),
        &menu_items.about,
        &PredefinedMenuItem::separator(),
        &menu_items.quit,
    ]);
    let ids = (
        menu_items.open_app.id().clone(),
        menu_items.open_tray.id().clone(),
        menu_items.refresh.id().clone(),
        menu_items.about.id().clone(),
        menu_items.quit.id().clone(),
    );

    apply_l10n(initial_lang, &tray_ui, &app_ui, &about_window, &menu_items);
    apply_theme(initial_theme, &tray_ui, &app_ui, &about_window);
    render_all(&tray_ui, &app_ui, &panel.borrow());
    push_settings_all(&app_ui, &mut panel.borrow_mut());

    let start_visible_timer: Rc<dyn Fn()> = {
        let tray_weak = tray_ui.as_weak();
        let app_weak = app_ui.as_weak();
        let panel = panel.clone();
        let timer = visible_timer.clone();
        Rc::new(move || {
            let tray_weak = tray_weak.clone();
            let app_weak = app_weak.clone();
            let panel = panel.clone();
            let timer_stop = timer.clone();
            timer.start(slint::TimerMode::Repeated, VISIBLE_TICK, move || {
                let (Some(tray_ui), Some(app_ui)) = (tray_weak.upgrade(), app_weak.upgrade()) else {
                    timer_stop.stop();
                    return;
                };
                let tray_vis = tray_ui.window().is_visible();
                let app_vis = app_ui.window().is_visible();
                if !tray_vis && !app_vis {
                    timer_stop.stop();
                    return;
                }
                let mut p = panel.borrow_mut();
                render_all(&tray_ui, &app_ui, &p);

                #[cfg(windows)]
                {
                    let debug_pin = cfg!(debug_assertions) && std::env::var_os("QPEEK_DEBUG").is_some();
                    if !debug_pin
                        && tray_vis
                        && p.shown_at.is_some_and(|t| t.elapsed() > OPEN_GRACE)
                        && let Some(hwnd) = win::hwnd_of(tray_ui.window())
                        && !win::is_foreground(hwnd)
                    {
                        hide_tray(&tray_ui, &app_ui, &mut p, &timer_stop);
                    }
                    if !debug_pin
                        && app_vis
                        && p.app_shown_at.is_some_and(|t| t.elapsed() > OPEN_GRACE)
                        && let Some(hwnd) = win::hwnd_of(app_ui.window())
                        && !win::is_foreground(hwnd)
                    {
                        hide_app(&tray_ui, &app_ui, &mut p, &timer_stop);
                    }
                }
            });
        })
    };

    // AppWindow kapatma isteğinde gizlenir, tepsi çalışmaya devam eder.
    {
        let tray_weak = tray_ui.as_weak();
        let app_weak = app_ui.as_weak();
        let panel = panel.clone();
        let timer = visible_timer.clone();
        app_ui.window().on_close_requested(move || {
            if let (Some(tray_ui), Some(app_ui)) = (tray_weak.upgrade(), app_weak.upgrade()) {
                hide_app(&tray_ui, &app_ui, &mut panel.borrow_mut(), &timer);
            }
            CloseRequestResponse::HideWindow
        });
    }

    {
        let bus = bus.clone();
        TrayIconEvent::set_event_handler(Some(move |ev| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                bus.send(AppEvent::ToggleTray);
            }
        }));
    }
    {
        let bus = bus.clone();
        MenuEvent::set_event_handler(Some(move |ev: MenuEvent| {
            bus.send(AppEvent::Menu(ev.id));
        }));
    }
    let tray = Rc::new(
        TrayIconBuilder::new()
            .with_tooltip("QPeek")
            .with_icon(tray_icon_for(None))
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .ok(),
    );

    // Ortak geri çağrılar (hem tepsi paneli hem geniş pencere).
    let on_refresh_fn = {
        let tx = refresh_tx.clone();
        Rc::new(move || {
            let _ = tx.send(());
        })
    };
    {
        let f = on_refresh_fn.clone();
        tray_ui.on_refresh(move || f());
    }
    {
        let tray_weak = tray_ui.as_weak();
        let app_weak = app_ui.as_weak();
        let panel = panel.clone();
        let timer = visible_timer.clone();
        tray_ui.on_dismiss(move || {
            if let (Some(t), Some(a)) = (tray_weak.upgrade(), app_weak.upgrade()) {
                hide_tray(&t, &a, &mut panel.borrow_mut(), &timer);
            }
        });
    }
    {
        let tray_weak = tray_ui.as_weak();
        let app_weak = app_ui.as_weak();
        let panel = panel.clone();
        let timer = visible_timer.clone();
        app_ui.on_dismiss(move || {
            if let (Some(t), Some(a)) = (tray_weak.upgrade(), app_weak.upgrade()) {
                hide_app(&t, &a, &mut panel.borrow_mut(), &timer);
            }
        });
    }

    {
        let app_weak = app_ui.as_weak();
        let panel = panel.clone();
        let worker = settings_worker.clone();
        app_ui.on_refresh(move || {
            on_refresh_fn();
            if let Some(a) = app_weak.upgrade()
                && a.get_settings_tab() == 0
            {
                refresh_settings(&a, &mut panel.borrow_mut(), &worker);
            }
        });
    }
    {
        let app_weak = app_ui.as_weak();
        let panel = panel.clone();
        let cost_tx = cost_tx.clone();
        app_ui.on_select_cost_days(move |days| {
            let Some(app) = app_weak.upgrade() else { return };
            if matches!(days, 1 | 7 | 30) {
                spawn_cost_fetch(days as u32, &cost_tx, &mut panel.borrow_mut(), &app);
            }
        });
    }
    {
        let app_weak = app_ui.as_weak();
        let panel = panel.clone();
        app_ui.on_refresh_cost(move || {
            let Some(app) = app_weak.upgrade() else { return };
            let mut p = panel.borrow_mut();
            spawn_cost_fetch(p.cost_days, &cost_tx, &mut p, &app);
        });
    }
    {
        let tray_weak = tray_ui.as_weak();
        let app_weak = app_ui.as_weak();
        let panel = panel.clone();
        let timer = visible_timer.clone();
        let start_timer = start_visible_timer.clone();
        let card_models = card_models.clone();
        let workers = app_workers.clone();
        tray_ui.on_open_providers(move || {
            let (Some(t), Some(a)) = (tray_weak.upgrade(), app_weak.upgrade()) else { return };
            let mut p = panel.borrow_mut();
            hide_tray(&t, &a, &mut p, &timer);
            p.search_query.clear();
            p.filter_mode = 0;
            a.set_search_text("".into());
            a.set_filter_mode(0);
            a.set_settings_tab(0);
            show_app(&t, &a, &card_models, &mut p, &start_timer, &workers);
        });
    }

    {
        let app_weak = app_ui.as_weak();
        let panel = panel.clone();
        app_ui.on_search_changed(move |query| {
            let Some(a) = app_weak.upgrade() else { return };
            let mut p = panel.borrow_mut();
            p.search_query = query.to_string();
            push_settings_all(&a, &mut p);
        });
    }
    {
        let app_weak = app_ui.as_weak();
        let panel = panel.clone();
        app_ui.on_set_filter(move |mode| {
            let Some(a) = app_weak.upgrade() else { return };
            let mut p = panel.borrow_mut();
            p.filter_mode = mode;
            a.set_filter_mode(mode);
            push_settings_all(&a, &mut p);
        });
    }
    {
        let app_weak = app_ui.as_weak();
        let panel = panel.clone();
        app_ui.on_select_provider(move |provider| {
            let Some(a) = app_weak.upgrade() else { return };
            let mut p = panel.borrow_mut();
            p.selected_provider_id = provider.to_string();
            a.set_api_key_draft("".into());
            push_settings_all(&a, &mut p);
        });
    }

    let on_provider_action = {
        let app_weak = app_ui.as_weak();
        let panel = panel.clone();
        let worker = settings_worker.clone();
        Rc::new(move |provider: slint::SharedString, action| {
            let Some(a) = app_weak.upgrade() else { return };
            let provider = provider.to_string();
            let mut p = panel.borrow_mut();
            if p.busy.contains_key(&provider) {
                return;
            }
            let busy = match action {
                ProviderAction::AddAccount => ProviderBusy::SigningIn,
                ProviderAction::SaveApiKey(_) => ProviderBusy::SavingKey,
                _ => ProviderBusy::Working,
            };
            p.busy.insert(provider.clone(), busy);
            p.notes.remove(&provider);
            push_settings_all(&a, &mut p);
            worker.spawn(Some((provider, action)));
            if matches!(busy, ProviderBusy::SavingKey) {
                a.set_api_key_draft("".into());
            }
        })
    };
    {
        let f = on_provider_action.clone();
        app_ui.on_account_action(move |provider, key, connect| {
            f(provider, ProviderAction::Account { key: key.to_string(), connect });
        });
    }
    {
        let f = on_provider_action.clone();
        app_ui.on_add_account(move |provider| f(provider, ProviderAction::AddAccount));
    }
    {
        let f = on_provider_action.clone();
        app_ui.on_toggle_provider(move |provider, enabled| f(provider, ProviderAction::Toggle(enabled)));
    }
    app_ui.on_save_api_key(move |provider, key| on_provider_action(provider, ProviderAction::SaveApiKey(key.to_string())));
    wire_activation_callbacks(&app_ui, &panel, &bus);

    let on_switch_fn = {
        let card_models = card_models.clone();
        let panel = panel.clone();
        let tray = tray.clone();
        Rc::new(move |provider: slint::SharedString, delta: i32, is_wheel: bool| {
            let mut p = panel.borrow_mut();
            if is_wheel {
                if p.last_wheel.is_some_and(|t| t.elapsed() < WHEEL_COOLDOWN) {
                    return;
                }
                p.last_wheel = Some(Instant::now());
            }
            let keys: Vec<String> = p
                .cards
                .iter()
                .filter(|c| c.id == provider.as_str())
                .map(|c| c.account_key.clone())
                .collect();
            if keys.len() < 2 {
                return;
            }
            let current = selected_index(&p, provider.as_str(), &keys);
            let next = (current as i32 + delta).clamp(0, (keys.len() - 1) as i32) as usize;
            if next == current {
                return;
            }
            p.selected.insert(provider.to_string(), keys[next].clone());
            save_selection(&p.selected);
            update_card_index(&card_models, provider.as_str(), next as i32);
            if let Some(tr) = tray.as_ref().as_ref() {
                let _ = tr.set_tooltip(Some(tooltip(&p)));
            }
        })
    };
    {
        let f = on_switch_fn.clone();
        tray_ui.on_switch_account(move |p, d, w| f(p, d, w));
    }
    {
        let f = on_switch_fn.clone();
        app_ui.on_switch_account(move |p, d, w| f(p, d, w));
    }

    let on_autostart_fn = {
        let tray_weak = tray_ui.as_weak();
        let app_weak = app_ui.as_weak();
        let panel = panel.clone();
        Rc::new(move |enabled: bool| {
            let (Some(t), Some(a)) = (tray_weak.upgrade(), app_weak.upgrade()) else { return };
            let mut p = panel.borrow_mut();
            #[cfg(windows)]
            match win::set_autostart(enabled) {
                Ok(()) => {
                    p.autostart = enabled;
                    t.set_settings_error("".into());
                    a.set_settings_error("".into());
                }
                Err(e) => {
                    t.set_settings_error(e.clone().into());
                    a.set_settings_error(e.into());
                }
            }
            #[cfg(not(windows))]
            {
                p.autostart = enabled;
            }
            render_all(&t, &a, &p);
        })
    };
    {
        let f = on_autostart_fn.clone();
        tray_ui.on_toggle_autostart(move |v| f(v));
    }
    {
        let f = on_autostart_fn.clone();
        app_ui.on_toggle_autostart(move |v| f(v));
    }

    let on_alerts_fn = {
        let tray_weak = tray_ui.as_weak();
        let app_weak = app_ui.as_weak();
        let panel = panel.clone();
        let prefs = prefs.clone();
        Rc::new(move |enabled: bool| {
            let (Some(t), Some(a)) = (tray_weak.upgrade(), app_weak.upgrade()) else { return };
            let mut p = panel.borrow_mut();
            p.alerts_enabled = enabled;
            let mut prefs = prefs.borrow_mut();
            prefs.alerts_enabled = enabled;
            save_prefs(&prefs);
            render_all(&t, &a, &p);
        })
    };
    {
        let f = on_alerts_fn.clone();
        tray_ui.on_toggle_alerts(move |v| f(v));
    }
    {
        let f = on_alerts_fn.clone();
        app_ui.on_toggle_alerts(move |v| f(v));
    }

    let on_test_alert_fn = {
        let panel = panel.clone();
        Rc::new(move || {
            let lang = panel.borrow().lang;
            let (title, body) = notify::test_toast(lang);
            #[cfg(windows)]
            win::send_toast(title, body);
            #[cfg(not(windows))]
            let _ = (title, body);
        })
    };
    {
        let f = on_test_alert_fn.clone();
        tray_ui.on_test_alert(move || f());
    }
    {
        let f = on_test_alert_fn.clone();
        app_ui.on_test_alert(move || f());
    }

    let on_set_lang_fn = {
        let tray_weak = tray_ui.as_weak();
        let app_weak = app_ui.as_weak();
        let about_window = about_window.clone();
        let menu_items = menu_items.clone();
        let card_models = card_models.clone();
        let panel = panel.clone();
        let tray = tray.clone();
        let prefs = prefs.clone();
        Rc::new(move |code: slint::SharedString| {
            let (Some(t), Some(a)) = (tray_weak.upgrade(), app_weak.upgrade()) else { return };
            let new_lang = Lang::from_code(code.as_str());
            let mut p = panel.borrow_mut();
            if p.lang == new_lang {
                return;
            }
            p.lang = new_lang;
            let mut prefs = prefs.borrow_mut();
            prefs.lang = new_lang;
            save_prefs(&prefs);
            apply_l10n(new_lang, &t, &a, &about_window, &menu_items);
            set_cards_all(&card_models, &mut p);
            push_settings_all(&a, &mut p);
            push_cost(&a, &p);
            render_all(&t, &a, &p);
            if let Some(tr) = tray.as_ref().as_ref() {
                let _ = tr.set_tooltip(Some(tooltip(&p)));
            }
        })
    };
    {
        let f = on_set_lang_fn.clone();
        tray_ui.on_set_lang(move |c| f(c));
    }
    {
        let f = on_set_lang_fn.clone();
        app_ui.on_set_lang(move |c| f(c));
    }

    let on_set_theme_fn = {
        let tray_weak = tray_ui.as_weak();
        let app_weak = app_ui.as_weak();
        let about_window = about_window.clone();
        let card_models = card_models.clone();
        let panel = panel.clone();
        Rc::new(move |code: slint::SharedString| {
            let (Some(t), Some(a)) = (tray_weak.upgrade(), app_weak.upgrade()) else { return };
            let id = ThemeId::from_code(code.as_str());
            let mut p = panel.borrow_mut();
            if p.theme == id {
                return;
            }
            p.theme = id;
            let mut prefs = prefs.borrow_mut();
            prefs.theme = id;
            save_prefs(&prefs);
            apply_theme(id, &t, &a, &about_window);
            set_cards_all(&card_models, &mut p);
            push_settings_all(&a, &mut p);
            push_cost(&a, &p);
            render_all(&t, &a, &p);
        })
    };
    {
        let f = on_set_theme_fn.clone();
        tray_ui.on_set_theme(move |code| f(code));
    }
    app_ui.on_set_theme(move |code| on_set_theme_fn(code));

    // Olay kuyruğunu ana iş parçacığında boşaltan dağıtıcı.
    {
        let tray_weak = tray_ui.as_weak();
        let app_weak = app_ui.as_weak();
        let about_window = about_window.clone();
        let card_models = card_models.clone();
        let panel = panel.clone();
        let tray = tray.clone();
        let timer = visible_timer.clone();
        let start_timer = start_visible_timer.clone();
        let refresh_tx = refresh_tx.clone();
        let workers = app_workers.clone();
        let bus = bus.clone();
        let (id_open_app, id_open_tray, id_refresh, id_about, id_quit) = ids;
        let drain: Rc<dyn Fn()> = Rc::new(move || {
            let (Some(tray_ui), Some(app_ui)) = (tray_weak.upgrade(), app_weak.upgrade()) else { return };
            let mut p = panel.borrow_mut();
            for ev in event_rx.try_iter().chain(cost_rx.try_iter().map(AppEvent::Update)) {
                match ev {
                    AppEvent::ToggleTray => {
                        if tray_ui.window().is_visible() {
                            hide_tray(&tray_ui, &app_ui, &mut p, &timer);
                        } else if p.hidden_at.is_none_or(|t| t.elapsed() > REOPEN_GUARD) {
                            show_tray(&tray_ui, &app_ui, &card_models, &mut p, &start_timer, &refresh_tx);
                        }
                    }
                    AppEvent::OpenApp => {
                        if app_ui.window().is_visible() {
                            hide_app(&tray_ui, &app_ui, &mut p, &timer);
                        } else if p.app_hidden_at.is_none_or(|t| t.elapsed() > REOPEN_GUARD) {
                            if tray_ui.window().is_visible() {
                                hide_tray(&tray_ui, &app_ui, &mut p, &timer);
                            }
                            show_app(&tray_ui, &app_ui, &card_models, &mut p, &start_timer, &workers);
                        }
                    }
                    AppEvent::Menu(id) => {
                        if id == id_open_app {
                            if tray_ui.window().is_visible() {
                                hide_tray(&tray_ui, &app_ui, &mut p, &timer);
                            }
                            show_app(&tray_ui, &app_ui, &card_models, &mut p, &start_timer, &workers);
                        } else if id == id_open_tray {
                            show_tray(&tray_ui, &app_ui, &card_models, &mut p, &start_timer, &refresh_tx);
                        } else if id == id_refresh {
                            let _ = refresh_tx.send(());
                        } else if id == id_about {
                            show_about(&about_window, p.lang, p.theme);
                        } else if id == id_quit {
                            let _ = slint::quit_event_loop();
                        }
                    }
                    AppEvent::Update(update) => match update {
                        Update::Started => {
                            p.refreshing = true;
                            if tray_ui.window().is_visible() || app_ui.window().is_visible() {
                                render_all(&tray_ui, &app_ui, &p);
                            }
                        }
                        Update::Done(result) => {
                            p.refreshing = false;
                            match result {
                                Ok(cards) => {
                                    p.peak = cards.iter().flat_map(|c| c.bars().map(|m| m.percent)).reduce(f32::max);
                                    p.cards = cards;
                                    p.fetched_at = Some(Instant::now());
                                    p.error = None;
                                    let toasts = {
                                        let lang = p.lang;
                                        let on = p.alerts_enabled;
                                        let cards_snapshot = p.cards.clone();
                                        p.alerts.observe(&cards_snapshot, lang, on)
                                    };
                                    #[cfg(windows)]
                                    for (title, body) in toasts {
                                        win::send_toast(title, body);
                                    }
                                    #[cfg(not(windows))]
                                    let _ = toasts;
                                    if let Some(t) = tray.as_ref().as_ref() {
                                        update_tray(t, &p);
                                    }
                                    set_cards_all(&card_models, &mut p);
                                    let now_local = chrono::Local::now();
                                    let now_local_mins = now_local.hour() * 60 + now_local.minute();
                                    let now_utc_unix = chrono::Utc::now().timestamp();
                                    let accounts: Vec<_> = p.cards.iter().map(|card| {
                                        (
                                            card.id.clone(),
                                            card.account_key.clone(),
                                            card.has_5h_window(),
                                            card.primary_5h_percent(),
                                        )
                                    }).collect();
                                    for (id, key, has_5h, pct) in accounts {
                                        if activation::should_auto_trigger(
                                            &p.activation, &id, &key, has_5h, pct, now_local_mins, now_utc_unix,
                                        ) {
                                            spawn_activation_trigger(&mut p, &bus, id, key);
                                        }
                                    }
                                    if !p.providers.is_empty() {
                                        push_settings_all(&app_ui, &mut p);
                                    }
                                }
                                Err(e) => p.error = Some(e),
                            }
                            if tray_ui.window().is_visible() || app_ui.window().is_visible() {
                                render_all(&tray_ui, &app_ui, &p);
                            }
                        }
                        Update::CostDone(days, result) => {
                            p.cost_pending.remove(&days);
                            if days != p.cost_days {
                                continue;
                            }
                            p.cost_loading = false;
                            match result {
                                Ok(data) => {
                                    p.cost_data = Some(data);
                                    p.cost_error = None;
                                }
                                Err(error) => {
                                    p.cost_data = None;
                                    p.cost_error = Some(error);
                                }
                            }
                            push_cost(&app_ui, &p);
                        }
                        Update::ActivationDone(provider, key, result) => {
                            let acc_id = activation::account_id(&provider, &key);
                            p.act_busy.remove(&acc_id);
                            let hhmm = chrono::Local::now().format("%H:%M").to_string();
                            let started = if p.lang.is_en() { "Started" } else { "Başlatıldı" };
                            let account = p.activation.accounts.entry(acc_id).or_default();
                            match result {
                                Ok(()) => {
                                    account.last_triggered_unix = Some(chrono::Utc::now().timestamp());
                                    account.last_status = format!("{hhmm} · {started}");
                                    #[cfg(windows)]
                                    if p.alerts_enabled {
                                        let card = p.cards.iter().find(|c| c.id == provider && c.account_key == key);
                                        let name = card.map_or(provider.as_str(), |c| c.name.as_str());
                                        let who = card.filter(|c| !c.email.is_empty())
                                            .map_or(key.as_str(), |c| c.email.as_str());
                                        let title = i18n::ui_texts(p.lang).activation_title.into();
                                        win::send_toast(title, format!("{name} · {who} · {started}"));
                                    }
                                    let _ = refresh_tx.send(());
                                }
                                Err(e) => account.last_status = format!("{hhmm} · {e}"),
                            }
                            activation::save(&p.activation);
                            push_settings_all(&app_ui, &mut p);
                        }
                        Update::SettingsData { providers, action } => {
                            app_ui.set_settings_loading(false);
                            match providers {
                                Ok(providers) => {
                                    p.providers = providers;
                                    app_ui.set_settings_error("".into());
                                }
                                Err(e) => app_ui.set_settings_error(e.into()),
                            }
                            if let Some((provider, result)) = action {
                                p.busy.remove(&provider);
                                if let Err(e) = result {
                                    p.notes.insert(provider, e);
                                }
                                let _ = refresh_tx.send(());
                            }
                            push_settings_all(&app_ui, &mut p);
                        }
                    },
                }
            }
        });
        DRAIN.with(|d| *d.borrow_mut() = Some(drain.clone()));
        drain();
    }

    let show_startup_app = || {
        show_app(
            &tray_ui,
            &app_ui,
            &card_models,
            &mut panel.borrow_mut(),
            &start_visible_timer,
            &app_workers,
        );
    };
    match debug_mode.as_deref() {
        Some("1") => show_tray(&tray_ui, &app_ui, &card_models, &mut panel.borrow_mut(), &start_visible_timer, &refresh_tx),
        Some("settings") => {
            show_tray(&tray_ui, &app_ui, &card_models, &mut panel.borrow_mut(), &start_visible_timer, &refresh_tx);
            tray_ui.set_settings_open(true);
        }
        Some("app" | "app-en" | "app-settings") => show_startup_app(),
        Some("app-apikey" | "app-settings-apikey") => {
            panel.borrow_mut().selected_provider_id = "openrouter".into();
            show_startup_app();
        }
        Some("app-general") => {
            show_startup_app();
            app_ui.set_settings_tab(2);
        }
        Some("app-cost") => {
            app_ui.set_settings_tab(1);
            show_startup_app();
        }
        _ if !start_hidden => show_startup_app(),
        _ => {}
    }

    let res = slint::run_event_loop_until_quit();
    DRAIN.with(|d| *d.borrow_mut() = None);
    res
}

fn color_rgb((r, g, b): (u8, u8, u8)) -> Color {
    Color::from_rgb_u8(r, g, b)
}

fn apply_theme_on_global(g: &Theme<'_>, id: ThemeId) {
    let spec = id.spec();
    g.set_active_theme(spec.id.as_code().into());
    g.set_is_light(spec.is_light);
    g.set_font_family(spec.font_family.into());
    g.set_card_radius(spec.card_radius);
    g.set_control_radius(spec.control_radius);
    g.set_bar_radius(spec.bar_radius);
    g.set_bar_height(spec.bar_height);
    g.set_border_w(spec.border_w);
    g.set_bg(color_rgb(spec.bg));
    g.set_stub_bg(color_rgb(spec.stub_bg));
    g.set_notch_bg(color_rgb(spec.notch_bg));
    g.set_perf_dash(color_rgb(spec.perf_dash));
    g.set_card(color_rgb(spec.card));
    g.set_card_border(color_rgb(spec.card_border));
    g.set_hover_bg(color_rgb(spec.hover_bg));
    g.set_badge_bg(color_rgb(spec.badge_bg));
    g.set_badge_border(color_rgb(spec.badge_border));
    g.set_track(color_rgb(spec.track));
    g.set_monogram_bg(color_rgb(spec.monogram_bg));
    g.set_text_primary(color_rgb(spec.text_primary));
    g.set_text_secondary(color_rgb(spec.text_secondary));
    g.set_text_muted(color_rgb(spec.text_muted));
    g.set_text_dim(color_rgb(spec.text_dim));
    g.set_emerald(color_rgb(spec.emerald));
    g.set_amber(color_rgb(spec.amber));
    g.set_coral(color_rgb(spec.coral));
    g.set_sky(color_rgb(spec.sky));
    g.set_active_btn_text(color_rgb(spec.active_btn_text));
}

fn apply_theme(
    id: ThemeId,
    tray_ui: &MainWindow,
    app_ui: &AppWindow,
    about: &Rc<RefCell<Option<AboutWindow>>>,
) {
    apply_theme_on_global(&tray_ui.global::<Theme>(), id);
    apply_theme_on_global(&app_ui.global::<Theme>(), id);
    #[cfg(windows)]
    let spec = id.spec();
    #[cfg(windows)]
    if let Some(hwnd) = win::hwnd_of(tray_ui.window()) {
        win::set_rounded_border(hwnd, spec.card_border);
    }
    if app_ui.window().is_visible() {
        apply_app_window_shape(app_ui, id);
    }
    if let Some(w) = about.borrow().as_ref() {
        apply_theme_on_global(&w.global::<Theme>(), id);
        #[cfg(windows)]
        if w.window().is_visible() && let Some(hwnd) = win::hwnd_of(w.window()) {
            win::set_titlebar_colors(hwnd, !spec.is_light, spec.bg, spec.text_primary);
        }
    }
}

fn apply_l10n_on_global(g: &L10n<'_>, lang: Lang) {
    let t = i18n::ui_texts(lang);
    g.set_is_en(lang.is_en());
    g.set_settings(t.settings.into());
    g.set_general(t.general.into());
    g.set_providers_tab(t.providers_tab.into());
    g.set_manage_providers_title(t.manage_providers_title.into());
    g.set_manage_providers_sub(t.manage_providers_sub.into());
    g.set_search_placeholder(t.search_placeholder.into());
    g.set_filter_all(t.filter_all.into());
    g.set_filter_connected(t.filter_connected.into());
    g.set_no_search_results(t.no_search_results.into());
    g.set_accounts_section(t.accounts_section.into());
    g.set_connected_badge(t.connected_badge.into());
    g.set_auth_multi_account(t.auth_multi_account.into());
    g.set_auth_local_account(t.auth_local_account.into());
    g.set_auth_api_key(t.auth_api_key.into());
    g.set_auth_optional_key(t.auth_optional_key.into());
    g.set_auth_auto_detect(t.auth_auto_detect.into());
    g.set_api_key_section(t.api_key_section.into());
    g.set_optional_api_key_section(t.optional_api_key_section.into());
    g.set_update_key_btn(t.update_key_btn.into());
    g.set_auto_detect_desc(t.auto_detect_desc.into());
    g.set_autostart(t.autostart.into());
    g.set_notifications(t.notifications.into());
    g.set_test_btn(t.test_btn.into());
    g.set_language(t.language.into());
    g.set_design_language(t.design_language.into());
    g.set_theme_obsidian(t.theme_obsidian.into());
    g.set_theme_paper(t.theme_paper.into());
    g.set_theme_brutalist(t.theme_brutalist.into());
    g.set_theme_terminal(t.theme_terminal.into());
    g.set_theme_nordic(t.theme_nordic.into());
    g.set_theme_hint(t.theme_hint.into());
    g.set_hotkey_label(t.hotkey_label.into());
    g.set_shortcuts_title(t.shortcuts_title.into());
    g.set_shortcut_refresh(t.shortcut_refresh.into());
    g.set_shortcut_refresh_keys(t.shortcut_refresh_keys.into());
    g.set_shortcut_close(t.shortcut_close.into());
    g.set_shortcut_close_keys(t.shortcut_close_keys.into());
    g.set_shortcut_switch(t.shortcut_switch.into());
    g.set_shortcut_switch_keys(t.shortcut_switch_keys.into());
    g.set_limits_title(t.limits_title.into());
    g.set_cost_tab(t.cost_tab.into());
    g.set_cost_total(t.cost_total.into());
    g.set_cost_sessions(t.cost_sessions.into());
    g.set_cost_tokens(t.cost_tokens.into());
    g.set_cost_loading(t.cost_loading.into());
    g.set_cost_empty(t.cost_empty.into());
    g.set_cost_note(t.cost_note.into());
    g.set_cost_1d(t.cost_1d.into());
    g.set_cost_7d(t.cost_7d.into());
    g.set_cost_30d(t.cost_30d.into());
    g.set_cost_daily_title(t.cost_daily_title.into());
    g.set_cost_models_title(t.cost_models_title.into());
    g.set_activation_title(t.activation_title.into());
    g.set_activation_master(t.activation_master.into());
    g.set_activation_mode_reset(t.activation_mode_reset.into());
    g.set_activation_mode_schedule(t.activation_mode_schedule.into());
    g.set_activation_start_time(t.activation_start_time.into());
    g.set_activation_quiet_hours(t.activation_quiet_hours.into());
    g.set_activation_account_toggle(t.activation_account_toggle.into());
    g.set_activation_follow_master(t.activation_follow_master.into());
    g.set_activation_start_now(t.activation_start_now.into());
    g.set_activation_hint(t.activation_hint.into());
    g.set_used_suffix(t.used_suffix.into());
    g.set_no_metrics(t.no_metrics.into());
    g.set_remove_btn(t.remove_btn.into());
    g.set_connect_btn(t.connect_btn.into());
    g.set_add_account_btn(t.add_account_btn.into());
    g.set_save_key_btn(t.save_key_btn.into());
    g.set_api_key_placeholder(t.api_key_placeholder.into());
    g.set_loading_accounts(t.loading_accounts.into());
    g.set_settings_footer(t.settings_footer.into());
    g.set_app_settings_footer(t.app_settings_footer.into());
    g.set_about_desc(t.about_desc.into());
    g.set_about_title(t.about_title.into());
}

fn apply_l10n(
    lang: Lang,
    tray_ui: &MainWindow,
    app_ui: &AppWindow,
    about: &Rc<RefCell<Option<AboutWindow>>>,
    menu: &MenuItems,
) {
    apply_l10n_on_global(&tray_ui.global::<L10n>(), lang);
    apply_l10n_on_global(&app_ui.global::<L10n>(), lang);
    if let Some(w) = about.borrow().as_ref() {
        apply_l10n_on_global(&w.global::<L10n>(), lang);
    }
    menu.set_lang(lang);
}

fn spawn_cost_fetch(days: u32, tx: &mpsc::Sender<Update>, state: &mut Panel, app: &AppWindow) {
    state.cost_days = days;
    state.cost_loading = true;
    state.cost_error = None;
    push_cost(app, state);

    // Reuse an in-flight scan when switching back to its date range.
    if state.cost_pending.insert(days) {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let result = backend::fetch_costs(days);
            if tx.send(Update::CostDone(days, result)).is_ok() {
                wake_event_loop();
            }
        });
    }
}

fn push_cost(app: &AppWindow, state: &Panel) {
    app.set_cost_days(state.cost_days as i32);
    app.set_cost_loading(state.cost_loading);
    app.set_cost_error(state.cost_error.as_deref().unwrap_or_default().into());

    let Some(summary) = state.cost_data.as_ref().filter(|s| s.days == state.cost_days) else {
        app.set_cost_total_usd("$0.00".into());
        app.set_cost_total_tokens("0".into());
        app.set_cost_total_sessions("0".into());
        app.set_cost_providers(ModelRc::default());
        app.set_cost_daily(ModelRc::default());
        return;
    };

    app.set_cost_total_usd(format!("${:.2}", summary.total_usd).into());
    app.set_cost_total_tokens(backend::format_tokens(summary.total_tokens).into());
    app.set_cost_total_sessions(summary.total_sessions.to_string().into());

    let texts = i18n::ui_texts(state.lang);
    let is_light = state.theme.spec().is_light;
    let providers: Vec<_> = summary
        .providers
        .iter()
        .map(|p| {
            let logo = logo_for(&mut state.logos.borrow_mut(), &p.id);
            let max_usd = p.models.iter().map(|m| m.usd).fold(0.0, f64::max);
            let models: Vec<_> = p
                .models
                .iter()
                .take(8)
                .map(|m| CostModelRow {
                    model: m.model.as_str().into(),
                    usd_text: format!("${:.2}", m.usd).into(),
                    ratio: cost_ratio(m.usd, max_usd),
                })
                .collect();
            CostProviderRow {
                id: p.id.as_str().into(),
                name: p.name.as_str().into(),
                accent: accent(&p.id, is_light),
                has_logo: logo.is_some(),
                logo: logo.unwrap_or_default(),
                monogram: monogram_for(&p.name).into(),
                total_text: format!("${:.2}", p.total_usd).into(),
                meta_text: format!(
                    "{} {} · {} {}",
                    p.sessions,
                    texts.cost_sessions,
                    backend::format_tokens(p.tokens),
                    texts.cost_tokens,
                )
                .into(),
                models: ModelRc::new(VecModel::from(models)),
            }
        })
        .collect();
    app.set_cost_providers(ModelRc::new(VecModel::from(providers)));

    let max_usd = summary.daily.iter().map(|d| d.usd).fold(0.0, f64::max);
    let daily: Vec<_> = summary
        .daily
        .iter()
        .map(|d| {
            let backend::DailyCost { day, usd, tokens: _tokens } = d;
            let label = if day.len() == 10 {
                day.get(5..).unwrap_or(day)
            } else {
                day
            };
            CostDailyRow {
                label: label.into(),
                usd_text: format!("${usd:.2}").into(),
                ratio: cost_ratio(*usd, max_usd),
            }
        })
        .collect();
    app.set_cost_daily(ModelRc::new(VecModel::from(daily)));
}

fn cost_ratio(usd: f64, max_usd: f64) -> f32 {
    if max_usd > 0.0 {
        (usd / max_usd).clamp(0.0, 1.0) as f32
    } else {
        0.0
    }
}

fn wire_activation_callbacks(app_ui: &AppWindow, panel: &Rc<RefCell<Panel>>, bus: &EventBus) {
    let app = app_ui.as_weak();
    app_ui.on_act_set_master_enabled(master_activation_callback(app.clone(), panel.clone(), |s, enabled| {
        s.enabled = enabled;
    }));
    app_ui.on_act_set_master_mode(master_activation_callback(app.clone(), panel.clone(), |s, mode| {
        s.mode = activation::ActivationMode::from_int(mode);
    }));
    app_ui.on_act_step_master_time(master_activation_callback(app.clone(), panel.clone(), |s, delta| {
        s.start_time = activation::step_hhmm(&s.start_time, delta);
    }));
    app_ui.on_act_set_master_quiet_enabled(master_activation_callback(app.clone(), panel.clone(), |s, enabled| {
        s.quiet_enabled = enabled;
    }));
    app_ui.on_act_step_master_quiet_start(master_activation_callback(app.clone(), panel.clone(), |s, delta| {
        s.quiet_start = activation::step_hhmm(&s.quiet_start, delta);
    }));
    app_ui.on_act_step_master_quiet_end(master_activation_callback(app.clone(), panel.clone(), |s, delta| {
        s.quiet_end = activation::step_hhmm(&s.quiet_end, delta);
    }));

    app_ui.on_act_set_account_enabled(account_activation_callback(app.clone(), panel.clone(), |s, enabled| {
        s.enabled = enabled;
    }));
    app_ui.on_act_set_account_follow(account_activation_callback(app.clone(), panel.clone(), |s, follow| {
        s.follow_master = follow;
    }));
    app_ui.on_act_set_account_mode(account_activation_callback(app.clone(), panel.clone(), |s, mode| {
        s.mode = activation::ActivationMode::from_int(mode);
    }));
    app_ui.on_act_step_account_time(account_activation_callback(app.clone(), panel.clone(), |s, delta| {
        s.start_time = activation::step_hhmm(&s.start_time, delta);
    }));
    app_ui.on_act_set_account_quiet(account_activation_callback(app.clone(), panel.clone(), |s, enabled| {
        s.quiet_enabled = enabled;
    }));
    app_ui.on_act_step_account_quiet_start(account_activation_callback(app.clone(), panel.clone(), |s, delta| {
        s.quiet_start = activation::step_hhmm(&s.quiet_start, delta);
    }));
    app_ui.on_act_step_account_quiet_end(account_activation_callback(app.clone(), panel.clone(), |s, delta| {
        s.quiet_end = activation::step_hhmm(&s.quiet_end, delta);
    }));

    let panel = panel.clone();
    let bus = bus.clone();
    app_ui.on_act_trigger_now(move |provider, key| {
        let Some(a) = app.upgrade() else { return };
        let mut p = panel.borrow_mut();
        if spawn_activation_trigger(&mut p, &bus, provider.to_string(), key.to_string()) {
            push_settings_all(&a, &mut p);
        }
    });
}

fn master_activation_callback<T>(
    app: slint::Weak<AppWindow>,
    panel: Rc<RefCell<Panel>>,
    edit: impl Fn(&mut activation::MasterSchedule, T) + 'static,
) -> impl Fn(T) + 'static {
    move |value| {
        let Some(a) = app.upgrade() else { return };
        let mut p = panel.borrow_mut();
        edit(&mut p.activation.master, value);
        activation::save(&p.activation);
        push_settings_all(&a, &mut p);
    }
}

fn account_activation_callback<T>(
    app: slint::Weak<AppWindow>,
    panel: Rc<RefCell<Panel>>,
    edit: impl Fn(&mut activation::AccountSchedule, T) + 'static,
) -> impl Fn(SharedString, SharedString, T) + 'static {
    move |provider, key, value| {
        let Some(a) = app.upgrade() else { return };
        let mut p = panel.borrow_mut();
        let account = p.activation.accounts.entry(activation::account_id(&provider, &key)).or_default();
        edit(account, value);
        activation::save(&p.activation);
        push_settings_all(&a, &mut p);
    }
}

fn spawn_activation_trigger(p: &mut Panel, bus: &EventBus, provider: String, key: String) -> bool {
    if !p.act_busy.insert(activation::account_id(&provider, &key)) {
        return false;
    }
    let bus = bus.clone();
    std::thread::spawn(move || {
        let result = activation::trigger_now(&provider, &key);
        bus.send(AppEvent::Update(Update::ActivationDone(provider, key, result)));
    });
    true
}

fn spawn_refresher(requests: Receiver<()>, bus: EventBus) {
    std::thread::spawn(move || loop {
        bus.send(AppEvent::Update(Update::Started));
        let result = backend::fetch();
        bus.send(AppEvent::Update(Update::Done(result)));
        match requests.recv_timeout(REFRESH_EVERY) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => while requests.try_recv().is_ok() {},
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    });
}

fn is_stale(p: &Panel) -> bool {
    !p.refreshing && p.fetched_at.is_none_or(|t| t.elapsed() > STALE_ON_OPEN)
}

fn render_all(tray_ui: &MainWindow, app_ui: &AppWindow, p: &Panel) {
    let (text, color) = status(p);
    let empty = if p.refreshing {
        if p.lang.is_en() { "Refreshing quota info..." } else { "Kota bilgileri yenileniyor..." }
    } else {
        let fallback = if p.lang.is_en() {
            "No quota data available. Right-click to refresh."
        } else {
            "Gösterilecek kota verisi bulunamadı. Yenilemek için sağ tıklayın."
        };
        p.error.as_deref().filter(|_| p.cards.is_empty()).unwrap_or(fallback)
    };

    if tray_ui.get_refreshing() != p.refreshing {
        tray_ui.set_refreshing(p.refreshing);
    }
    if tray_ui.get_status_text() != text.as_str() {
        tray_ui.set_status_text(text.as_str().into());
    }
    if tray_ui.get_status_color() != color {
        tray_ui.set_status_color(color);
    }
    if tray_ui.get_empty_text() != empty {
        tray_ui.set_empty_text(empty.into());
    }
    if tray_ui.get_autostart() != p.autostart {
        tray_ui.set_autostart(p.autostart);
    }
    if tray_ui.get_alerts_enabled() != p.alerts_enabled {
        tray_ui.set_alerts_enabled(p.alerts_enabled);
    }

    if app_ui.get_refreshing() != p.refreshing {
        app_ui.set_refreshing(p.refreshing);
    }
    if app_ui.get_status_text() != text.as_str() {
        app_ui.set_status_text(text.as_str().into());
    }
    if app_ui.get_status_color() != color {
        app_ui.set_status_color(color);
    }
    if app_ui.get_empty_text() != empty {
        app_ui.set_empty_text(empty.into());
    }
    if app_ui.get_autostart() != p.autostart {
        app_ui.set_autostart(p.autostart);
    }
    if app_ui.get_alerts_enabled() != p.alerts_enabled {
        app_ui.set_alerts_enabled(p.alerts_enabled);
    }
}

fn status(p: &Panel) -> (String, Color) {
    let spec = p.theme.spec();
    if p.refreshing {
        let s = if p.lang.is_en() { "Refreshing..." } else { "Yenileniyor..." };
        return (s.into(), color_rgb(spec.sky));
    }
    match p.fetched_at {
        None if p.error.is_some() => (if p.lang.is_en() { "Error" } else { "Hata" }.into(), color_rgb(spec.coral)),
        None => (if p.lang.is_en() { "Waiting" } else { "Bekleniyor" }.into(), color_rgb(spec.amber)),
        Some(t) => {
            let age = t.elapsed();
            if age > OLD_AFTER {
                let prefix = if p.lang.is_en() { "Stale" } else { "Eski" };
                (format!("{prefix} · {}", age_text(age, p.lang)), color_rgb(spec.amber))
            } else {
                let prefix = if p.lang.is_en() { "Fresh" } else { "Güncel" };
                (format!("{prefix} · {}", age_text(age, p.lang)), color_rgb(spec.emerald))
            }
        }
    }
}

fn age_text(age: Duration, lang: Lang) -> String {
    let s = age.as_secs();
    match (lang, s) {
        (Lang::Tr, 0..60) => format!("{s}sn önce"),
        (Lang::Tr, 60..3600) => format!("{}dk önce", s / 60),
        (Lang::Tr, 3600..86400) => format!("{}sa önce", s / 3600),
        (Lang::Tr, _) => format!("{}g önce", s / 86400),
        (Lang::En, 0..60) => format!("{s}s ago"),
        (Lang::En, 60..3600) => format!("{}m ago", s / 60),
        (Lang::En, 3600..86400) => format!("{}h ago", s / 3600),
        (Lang::En, _) => format!("{}d ago", s / 86400),
    }
}

fn provider_order(cards: &[Card]) -> Vec<String> {
    let mut order: Vec<String> = Vec::new();
    for c in cards {
        if !order.iter().any(|id| id == &c.id) {
            order.push(c.id.clone());
        }
    }
    order
}

fn slide_body_height(c: &Card) -> f32 {
    let email_h = if c.email.is_empty() { 0.0 } else { 15.0 + 8.0 };
    let metrics_h = if c.metrics.is_empty() {
        19.0
    } else {
        let rows: f32 = c.metrics.iter().map(|m| if m.info { 19.0 } else { 27.0 }).sum();
        let gaps = (c.metrics.len().saturating_sub(1) as f32) * 9.0;
        rows + gaps
    };
    email_h + metrics_h
}

fn sync_vec_model(model: &VecModel<ProviderCard>, items: Vec<ProviderCard>) {
    let same_layout = model.row_count() == items.len()
        && items
            .iter()
            .enumerate()
            .all(|(i, it)| model.row_data(i).is_some_and(|cur| cur.id == it.id));
    if same_layout {
        for (i, item) in items.into_iter().enumerate() {
            model.set_row_data(i, item);
        }
    } else {
        model.set_vec(items);
    }
}

fn update_card_index(models: &CardModels, provider: &str, new_index: i32) {
    for model in [&models.tray, &models.app] {
        for i in 0..model.row_count() {
            if let Some(mut row) = model.row_data(i)
                && row.id == provider
            {
                row.account_index = new_index;
                model.set_row_data(i, row);
            }
        }
    }
}

fn set_cards_all(models: &CardModels, p: &mut Panel) {
    let lang = p.lang;
    let is_light = p.theme.spec().is_light;
    let order = provider_order(&p.cards);
    let mut items = Vec::with_capacity(order.len());
    for id in order {
        let group: Vec<Card> = p.cards.iter().filter(|c| c.id == id).cloned().collect();
        if group.is_empty() {
            continue;
        }
        let keys: Vec<String> = group.iter().map(|c| c.account_key.clone()).collect();
        let index = selected_index(p, &id, &keys);
        let max_body_h = group.iter().map(slide_body_height).reduce(f32::max).unwrap_or(19.0);
        let logo = logo_for(p.logos.get_mut(), &id);
        let name = group[0].name.clone();
        let slides: Vec<AccountSlide> = group.iter().map(|c| to_slide(c, lang)).collect();
        items.push(ProviderCard {
            id: id.as_str().into(),
            account_index: index as i32,
            account_count: group.len() as i32,
            name: name.as_str().into(),
            monogram: monogram_for(&name).into(),
            accent: accent(&id, is_light),
            has_logo: logo.is_some(),
            logo: logo.unwrap_or_default(),
            min_body_height: max_body_h,
            slides: ModelRc::new(VecModel::from(slides)),
        });
    }
    sync_vec_model(&models.app, items.clone());
    sync_vec_model(&models.tray, items);
}

fn logo_for(logos: &mut HashMap<String, Option<slint::Image>>, id: &str) -> Option<slint::Image> {
    if let Some(cached) = logos.get(id) {
        return cached.clone();
    }
    let img = provider_icon_svg(id).and_then(|svg| slint::Image::load_from_svg_data(svg).ok());
    logos.insert(id.to_string(), img.clone());
    img
}

fn selected_index(p: &Panel, provider: &str, keys: &[String]) -> usize {
    p.selected
        .get(provider)
        .and_then(|k| keys.iter().position(|x| x == k))
        .unwrap_or(0)
}

fn appdata_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("APPDATA").map(|d| std::path::PathBuf::from(d).join("QPeek"))
}

fn state_file() -> Option<std::path::PathBuf> {
    appdata_dir().map(|d| d.join("selected.txt"))
}

fn prefs_file() -> Option<std::path::PathBuf> {
    appdata_dir().map(|d| d.join("prefs.txt"))
}

fn load_prefs() -> Prefs {
    let mut prefs = Prefs::default();
    if let Some(text) = prefs_file().and_then(|f| std::fs::read_to_string(f).ok()) {
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                match k.trim() {
                    "lang" => prefs.lang = Lang::from_code(v),
                    "alerts" => prefs.alerts_enabled = v.trim() != "0",
                    "theme" => prefs.theme = ThemeId::from_code(v),
                    _ => {}
                }
            }
        }
    }
    prefs
}

fn save_prefs(prefs: &Prefs) {
    let Some(file) = prefs_file() else { return };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let body = format!(
        "lang={}\nalerts={}\ntheme={}\n",
        prefs.lang.as_code(),
        if prefs.alerts_enabled { "1" } else { "0" },
        prefs.theme.as_code(),
    );
    let _ = std::fs::write(file, body);
}

fn load_selection() -> HashMap<String, String> {
    state_file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .map(|text| {
            text.lines()
                .filter_map(|l| l.split_once('='))
                .map(|(a, b)| (a.trim().to_string(), b.trim().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn save_selection(selected: &HashMap<String, String>) {
    let Some(file) = state_file() else { return };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text: String = selected.iter().map(|(a, b)| format!("{a}={b}\n")).collect();
    let _ = std::fs::write(file, text);
}

fn to_slide(c: &Card, lang: Lang) -> AccountSlide {
    let metrics: Vec<Metric> = c
        .metrics
        .iter()
        .map(|m| Metric {
            label: m.label(lang).into(),
            percent: m.percent,
            detail: m.detail(lang).into(),
            info: m.info,
            pace: m.pace(lang).into(),
            pace_level: m.pace_level,
        })
        .collect();
    AccountSlide {
        plan: c.plan.as_str().into(),
        email: c.email.as_str().into(),
        metrics: ModelRc::new(VecModel::from(metrics)),
    }
}

include!(concat!(env!("OUT_DIR"), "/provider_icons.rs"));

fn accent(id: &str, is_light: bool) -> Color {
    const DARK_FALLBACK: [(u8, u8, u8); 9] = [
        (0xFB, 0x71, 0x85),
        (0xFB, 0x92, 0x3C),
        (0xFA, 0xCC, 0x15),
        (0xA3, 0xE6, 0x35),
        (0x34, 0xD3, 0x99),
        (0x22, 0xD3, 0xEE),
        (0x60, 0xA5, 0xFA),
        (0xA7, 0x8B, 0xFA),
        (0xE8, 0x79, 0xF9),
    ];
    const LIGHT_FALLBACK: [(u8, u8, u8); 9] = [
        (0xBE, 0x12, 0x3C),
        (0x9A, 0x34, 0x12),
        (0x92, 0x55, 0x09),
        (0x3F, 0x62, 0x12),
        (0x04, 0x78, 0x57),
        (0x0E, 0x74, 0x90),
        (0x1D, 0x4E, 0xD8),
        (0x6D, 0x28, 0xD9),
        (0xA2, 0x1C, 0xAF),
    ];
    match id {
        "claude" => Color::from_rgb_u8(0xF9, 0x73, 0x16),
        "codex" => Color::from_rgb_u8(0x10, 0xB9, 0x81),
        "antigravity" => Color::from_rgb_u8(0x63, 0x66, 0xF1),
        "gemini" => Color::from_rgb_u8(0x3B, 0x82, 0xF6),
        "copilot" => Color::from_rgb_u8(0xA8, 0x55, 0xF7),
        "cursor" if is_light => Color::from_rgb_u8(0x27, 0x27, 0x2A),
        "cursor" => Color::from_rgb_u8(0xE5, 0xE7, 0xEB),
        "windsurf" => Color::from_rgb_u8(0x14, 0xB8, 0xA6),
        "openrouter" => Color::from_rgb_u8(0xEC, 0x48, 0x99),
        "deepseek" => Color::from_rgb_u8(0x38, 0xBD, 0xF8),
        _ => {
            let hash = id.bytes().fold(0u32, |hash, b| hash.wrapping_mul(31).wrapping_add(u32::from(b)));
            let palette = if is_light { LIGHT_FALLBACK } else { DARK_FALLBACK };
            color_rgb(palette[hash as usize % palette.len()])
        }
    }
}

fn update_tray(tray: &TrayIcon, p: &Panel) {
    let _ = tray.set_icon(Some(tray_icon_for(p.peak)));
    let _ = tray.set_tooltip(Some(tooltip(p)));
}

fn tooltip(p: &Panel) -> String {
    let mut lines = vec!["QPeek".to_string()];
    for id in provider_order(&p.cards) {
        let group: Vec<&Card> = p.cards.iter().filter(|c| c.id == id).collect();
        let keys: Vec<String> = group.iter().map(|c| c.account_key.clone()).collect();
        let index = selected_index(p, &id, &keys);
        let Some(c) = group.get(index) else { continue };
        let title = if group.len() > 1 {
            format!("{} ({}/{})", c.name, index + 1, group.len())
        } else {
            c.name.clone()
        };
        let line = if c.failed {
            let err = if p.lang.is_en() { "unavailable" } else { "veri alınamadı" };
            format!("{title}: {err}")
        } else {
            let parts: Vec<String> = c
                .bars()
                .map(|b| {
                    if p.lang.is_en() {
                        format!("{} {:.0}%", b.label(p.lang), b.percent)
                    } else {
                        format!("{} %{:.0}", b.label(p.lang), b.percent)
                    }
                })
                .collect();
            format!("{title}: {}", parts.join(" · "))
        };
        lines.push(line);
    }
    let text = lines.join("\n");
    if text.chars().count() > 127 {
        text.chars().take(126).collect::<String>() + "…"
    } else {
        text
    }
}

fn tray_icon_for(percent: Option<f32>) -> tray_icon::Icon {
    tray_icon::Icon::from_rgba(icon::gauge_rgba(percent), icon::SIZE, icon::SIZE).expect("ikon boyutu geçerli")
}

fn refresh_icon_frame(angle: f32) -> slint::Image {
    const FRAME_COUNT: usize = 60;
    const SVG: &str = include_str!("../assets/refresh-icon.svg");

    // Slint's software renderer ignores item rotation, so rotate the SVG itself.
    thread_local! {
        static FRAMES: [slint::Image; FRAME_COUNT] = std::array::from_fn(|frame| {
            let angle = frame as f32 * 360.0 / FRAME_COUNT as f32;
            let svg = SVG.replacen("rotate(0 12 12)", &format!("rotate({angle} 12 12)"), 1);
            slint::Image::load_from_svg_data(svg.as_bytes()).expect("Yenileme simgesi geçersiz")
        });
    }

    let frame = (angle * FRAME_COUNT as f32 / 360.0) as usize % FRAME_COUNT;
    FRAMES.with(|frames| frames[frame].clone())
}

fn show_tray(
    tray_ui: &MainWindow,
    app_ui: &AppWindow,
    models: &CardModels,
    p: &mut Panel,
    start_timer: &Rc<dyn Fn()>,
    refresh_tx: &Sender<()>,
) {
    set_cards_all(models, p);
    render_all(tray_ui, app_ui, p);
    let _ = tray_ui.show();
    place_tray(tray_ui);
    p.shown_at = Some(Instant::now());
    start_timer();
    #[cfg(windows)]
    if let Some(hwnd) = win::hwnd_of(tray_ui.window()) {
        win::set_rounded_border(hwnd, p.theme.spec().card_border);
        win::bring_to_front(hwnd);
    }
    if is_stale(p) {
        let _ = refresh_tx.send(());
    }
}

fn hide_tray(tray_ui: &MainWindow, app_ui: &AppWindow, p: &mut Panel, timer: &slint::Timer) {
    let _ = tray_ui.hide();
    if p.busy.is_empty() {
        tray_ui.set_settings_open(false);
    }
    p.hidden_at = Some(Instant::now());
    if !app_ui.window().is_visible() {
        timer.stop();
    }
}

fn apply_app_window_shape(app_ui: &AppWindow, theme: ThemeId) {
    #[cfg(windows)]
    if let Some(hwnd) = win::hwnd_of(app_ui.window()) {
        let size = app_ui.window().size();
        let scale = app_ui.window().scale_factor();
        let seam_x = (size.width as f32 - (470.0 + 12.0) * scale).round() as i32;
        let notch_r = (9.0 * scale).round().max(1.0) as i32;
        let corner_r = (theme.spec().card_radius.clamp(2.0, 12.0) * scale).round().max(1.0) as i32;
        win::apply_ticket_cutout(hwnd, size.width as i32, size.height as i32, seam_x, notch_r, corner_r);
        win::clear_window_border(hwnd);
    }
    #[cfg(not(windows))]
    let _ = (app_ui, theme);
}

fn show_app(
    tray_ui: &MainWindow,
    app_ui: &AppWindow,
    models: &CardModels,
    p: &mut Panel,
    start_timer: &Rc<dyn Fn()>,
    workers: &AppWorkers,
) {
    let was_visible = app_ui.window().is_visible();
    set_cards_all(models, p);
    render_all(tray_ui, app_ui, p);
    refresh_settings(app_ui, p, &workers.settings);
    if p.cost_data.is_none() && !p.cost_loading {
        spawn_cost_fetch(p.cost_days, &workers.cost, p, app_ui);
    }
    let _ = app_ui.show();
    if !was_visible {
        place_center(app_ui);
    }
    apply_app_window_shape(app_ui, p.theme);
    p.app_shown_at = Some(Instant::now());
    start_timer();
    #[cfg(windows)]
    if let Some(hwnd) = win::hwnd_of(app_ui.window()) {
        win::bring_to_front(hwnd);
    }
    if is_stale(p) {
        let _ = workers.refresh.send(());
    }
}

fn hide_app(tray_ui: &MainWindow, app_ui: &AppWindow, p: &mut Panel, timer: &slint::Timer) {
    let _ = app_ui.hide();
    if p.busy.is_empty() {
        app_ui.set_settings_tab(0);
    }
    p.app_hidden_at = Some(Instant::now());
    if !tray_ui.window().is_visible() {
        timer.stop();
    }
}

fn place_tray(ui: &MainWindow) {
    let size = ui.window().size();
    #[cfg(windows)]
    if let Some(s) = win::cursor_screen() {
        let (w, h) = (size.width as i32, size.height as i32);
        let margin = (MARGIN * ui.window().scale_factor()).round() as i32;
        let x = (s.cursor_x - w / 2)
            .min(s.work.right - margin - w)
            .max(s.work.left + margin);
        let taskbar_on_top = s.work.top > s.screen.top && s.work.bottom == s.screen.bottom;
        let y = if taskbar_on_top {
            s.work.top + margin
        } else {
            (s.work.bottom - margin - h).max(s.work.top + margin)
        };
        ui.window().set_position(slint::PhysicalPosition::new(x, y));
    }
}

fn place_center(ui: &AppWindow) {
    let size = ui.window().size();
    #[cfg(windows)]
    if let Some(s) = win::cursor_screen() {
        let (w, h) = (size.width as i32, size.height as i32);
        let x = s.work.left + ((s.work.right - s.work.left) - w) / 2;
        let y = s.work.top + ((s.work.bottom - s.work.top) - h) / 2;
        ui.window().set_position(slint::PhysicalPosition::new(x.max(s.work.left), y.max(s.work.top)));
    }
}

fn show_about(slot: &Rc<RefCell<Option<AboutWindow>>>, lang: Lang, theme: ThemeId) {
    let mut slot = slot.borrow_mut();
    if slot.is_none() {
        match AboutWindow::new() {
            Ok(w) => *slot = Some(w),
            Err(_) => return,
        }
    }
    if let Some(w) = slot.as_ref() {
        apply_l10n_on_global(&w.global::<L10n>(), lang);
        apply_theme_on_global(&w.global::<Theme>(), theme);
        let _ = w.show();
        #[cfg(windows)]
        if let Some(hwnd) = win::hwnd_of(w.window()) {
            let spec = theme.spec();
            win::set_titlebar_colors(hwnd, !spec.is_light, spec.bg, spec.text_primary);
        }
    }
}

fn refresh_settings(app_ui: &AppWindow, p: &mut Panel, worker: &SettingsWorker) {
    app_ui.set_settings_loading(p.providers.is_empty());
    app_ui.set_settings_error("".into());
    if !p.providers.is_empty() {
        push_settings_all(app_ui, p);
    }
    worker.spawn(None);
}

fn load_settings_data() -> Result<Vec<ProviderData>, String> {
    let states = backend::provider_states()?;
    let mut providers = Vec::with_capacity(states.len());
    for backend::CoreProvider { id, display_name, enabled, .. } in states {
        let can_add = accounts::supports_extra(&id);
        let mut rows = Vec::new();
        if can_add || id == "antigravity" {
            if accounts::default_logged_in(&id) {
                rows.push((accounts::DEFAULT_KEY.to_string(), accounts::default_email(&id), enabled));
            }
            for extra in accounts::extras(&id) {
                let email = accounts::extra_email(&extra);
                rows.push((extra.key, email, true));
            }
        }
        let (api_key_mode, api_env, api_help) = backend::api_key_meta(&id).map_or((0, "", ""), |meta| {
            (if meta.required { 1 } else { 2 }, meta.env_var, meta.help)
        });
        providers.push(ProviderData {
            id,
            display_name,
            connected: enabled,
            accounts: rows,
            can_add,
            api_key_mode,
            api_env: api_env.into(),
            api_help: api_help.into(),
        });
    }
    Ok(providers)
}

fn monogram_for(name: &str) -> String {
    let mut initials = name.split_whitespace().filter_map(|word| word.chars().find(|ch| ch.is_alphanumeric()));
    if let (Some(first), Some(second)) = (initials.next(), initials.next()) {
        return [first, second].into_iter().flat_map(char::to_uppercase).collect();
    }

    let letters = name.chars().filter(|ch| ch.is_alphanumeric());
    let capitals = letters.clone().filter(|ch| ch.is_uppercase());
    if capitals.clone().nth(1).is_some() {
        capitals.take(2).collect()
    } else {
        letters.take(2).flat_map(char::to_uppercase).collect()
    }
}

fn is_connected(d: &ProviderData) -> bool {
    d.connected || d.accounts.iter().any(|a| a.2)
}

fn visible_providers<'a>(providers: &'a [ProviderData], query: &str, filter_mode: i32) -> Vec<&'a ProviderData> {
    let query = query.to_lowercase();
    let mut visible: Vec<_> = providers
        .iter()
        .filter(|d| filter_mode != 1 || is_connected(d))
        .filter(|d| {
            query.is_empty()
                || d.display_name.to_lowercase().contains(&query)
                || d.id.to_lowercase().contains(&query)
        })
        .collect();
    visible.sort_by_cached_key(|d| (!is_connected(d), backend::provider_sort_rank(&d.id), d.display_name.to_lowercase()));
    visible
}

fn provider_detail(d: &ProviderData, row: &ProviderListRow, p: &Panel) -> ProviderDetail {
    let lang = p.lang;
    let texts = i18n::ui_texts(lang);
    let auth_label = match (d.can_add, d.id.as_str(), d.api_key_mode) {
        (true, _, _) => texts.auth_multi_account,
        (_, "antigravity", _) => texts.auth_local_account,
        (_, _, 1) => texts.auth_api_key,
        (_, _, 2) => texts.auth_optional_key,
        _ => texts.auth_auto_detect,
    };
    let default_schedule = activation::AccountSchedule::default();
    let accounts: Vec<AccountRow> = d
        .accounts
        .iter()
        .map(|(key, email, connected)| {
            let email = email.as_deref().unwrap_or(match (key.as_str(), d.id.as_str(), lang) {
                (accounts::DEFAULT_KEY, "antigravity", Lang::Tr) => "agy'de giriş yapılan hesap",
                (accounts::DEFAULT_KEY, "antigravity", Lang::En) => "Signed-in agy account",
                (accounts::DEFAULT_KEY, _, Lang::Tr) => "Bilgisayardaki hesap",
                (accounts::DEFAULT_KEY, _, Lang::En) => "Local signed-in account",
                (_, _, Lang::Tr) => "Ek hesap",
                (_, _, Lang::En) => "Extra account",
            });
            let has_5h_window = activation::supports_activation(&d.id)
                && p.cards.iter().any(|c| c.id == d.id && c.account_key == *key && c.has_5h_window());
            let acc_id = activation::account_id(&d.id, key);
            let acc = p.activation.accounts.get(&acc_id).unwrap_or(&default_schedule);
            let act_busy = p.act_busy.contains(&acc_id);
            let act_status = match (act_busy, lang) {
                (true, Lang::Tr) => "Başlatılıyor…",
                (true, Lang::En) => "Starting…",
                (false, _) => acc.last_status.as_str(),
            };
            AccountRow {
                key: key.as_str().into(),
                email: email.into(),
                connected: *connected,
                has_5h_window,
                act_enabled: acc.enabled,
                act_follow_master: acc.follow_master,
                act_mode: acc.mode.as_int(),
                act_time: acc.start_time.as_str().into(),
                act_quiet_enabled: acc.quiet_enabled,
                act_quiet_start: acc.quiet_start.as_str().into(),
                act_quiet_end: acc.quiet_end.as_str().into(),
                act_busy,
                act_status: act_status.into(),
            }
        })
        .collect();
    let busy = p.busy.get(&d.id).copied();
    let (note, note_kind) = if let Some(busy) = busy {
        (busy.text(lang), 1)
    } else if let Some(err) = p.notes.get(&d.id) {
        (err.as_str(), 2)
    } else if d.id == "antigravity" {
        let note = if lang.is_en() {
            "Antigravity supports a single account (agy sign-in)."
        } else {
            "Antigravity tek hesap destekliyor (agy'deki giriş)."
        };
        (note, 0)
    } else if d.can_add && d.accounts.is_empty() {
        (if lang.is_en() { "No connected account." } else { "Bağlı hesap yok." }, 0)
    } else {
        ("", 0)
    };
    ProviderDetail {
        id: row.id.clone(),
        name: row.name.clone(),
        monogram: row.monogram.clone(),
        accent: row.accent,
        logo: row.logo.clone(),
        has_logo: row.has_logo,
        connected: row.connected,
        accounts: ModelRc::new(VecModel::from(accounts)),
        can_add: d.can_add,
        api_key_mode: d.api_key_mode,
        api_env: d.api_env.as_str().into(),
        api_help: d.api_help.as_str().into(),
        auth_label: auth_label.into(),
        busy: busy.is_some(),
        note: note.into(),
        note_kind,
    }
}

fn push_settings_all(app_ui: &AppWindow, p: &mut Panel) {
    let master = &p.activation.master;
    app_ui.set_act_master_enabled(master.enabled);
    app_ui.set_act_master_mode(master.mode.as_int());
    app_ui.set_act_master_time(master.start_time.as_str().into());
    app_ui.set_act_master_quiet_enabled(master.quiet_enabled);
    app_ui.set_act_master_quiet_start(master.quiet_start.as_str().into());
    app_ui.set_act_master_quiet_end(master.quiet_end.as_str().into());

    let is_light = p.theme.spec().is_light;
    let visible = visible_providers(&p.providers, &p.search_query, p.filter_mode);
    let preferred_id = if p.selected_provider_id.is_empty() { "codex" } else { p.selected_provider_id.as_str() };
    let selected = visible
        .iter()
        .position(|d| d.id == preferred_id)
        .or_else(|| (!visible.is_empty()).then_some(0));
    if let Some(i) = selected {
        p.selected_provider_id.clone_from(&visible[i].id);
    }

    let rows: Vec<ProviderListRow> = visible
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let logo = logo_for(p.logos.get_mut(), &d.id);
            ProviderListRow {
                id: d.id.as_str().into(),
                name: d.display_name.as_str().into(),
                monogram: monogram_for(&d.display_name).into(),
                accent: accent(&d.id, is_light),
                has_logo: logo.is_some(),
                logo: logo.unwrap_or_default(),
                connected: is_connected(d),
                account_count: d.accounts.len() as i32,
                selected: selected == Some(i),
            }
        })
        .collect();
    app_ui.set_selected_provider(selected.map(|i| provider_detail(visible[i], &rows[i], p)).unwrap_or_default());
    app_ui.set_has_selection(selected.is_some());
    app_ui.set_provider_list(ModelRc::new(VecModel::from(rows)));
    app_ui.set_total_provider_count(p.providers.len() as i32);
    app_ui.set_connected_provider_count(p.providers.iter().filter(|d| is_connected(d)).count() as i32);
    app_ui.set_has_providers(!p.providers.is_empty());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_icon_rotates_clockwise() {
        let idle = refresh_icon_frame(0.0).to_rgba8().unwrap();
        let quarter_turn = refresh_icon_frame(90.0).to_rgba8().unwrap();
        let alpha = |image: &slint::SharedPixelBuffer<slint::Rgba8Pixel>, x: usize, y: usize| {
            image.as_slice()[y * image.width() as usize + x].a
        };

        assert_eq!(alpha(&idle, 19, 12), 0);
        assert!(alpha(&idle, 12, 19) > 0);
        assert!(alpha(&quarter_turn, 19, 12) > 0);
        assert_eq!(alpha(&quarter_turn, 12, 19), 0);
    }

    fn sample_card(id: &str, name: &str, key: &str, pct: f32) -> Card {
        Card {
            id: id.into(),
            name: name.into(),
            email: String::new(),
            account_key: key.into(),
            plan: String::new(),
            metrics: vec![backend::Metric {
                label_tr: "5 saat".into(),
                label_en: "5h".into(),
                percent: pct,
                detail_tr: String::new(),
                detail_en: String::new(),
                pace_tr: String::new(),
                pace_en: String::new(),
                pace_level: 0,
                info: false,
                window_minutes: Some(300),
            }],
            failed: false,
        }
    }

    #[test]
    fn tooltip_shows_selected_account_per_provider() {
        let mut p = Panel::default();
        p.cards = vec![
            sample_card("codex", "Codex", "default", 10.0),
            sample_card("codex", "Codex", "extra1", 55.0),
            sample_card("claude", "Claude", "default", 20.0),
        ];
        p.selected.insert("codex".into(), "extra1".into());
        assert_eq!(tooltip(&p), "QPeek\nCodex (2/2): 5 saat %55\nClaude: 5 saat %20");
        p.lang = Lang::En;
        assert_eq!(tooltip(&p), "QPeek\nCodex (2/2): 5h 55%\nClaude: 5h 20%");
    }

    #[test]
    fn monograms_use_word_initials_or_capitals() {
        for (name, expected) in [
            ("OpenAI API", "OA"),
            ("Hugging Face", "HF"),
            ("AWS Bedrock", "AB"),
            ("Vercel AI Gateway", "VA"),
            ("DeepSeek", "DS"),
            ("OpenRouter", "OR"),
            ("Grok", "GR"),
            ("v0", "V0"),
            ("Pi", "PI"),
            ("z.ai", "ZA"),
            ("ai&", "AI"),
            ("&hugging   -face", "HF"),
            ("éclair", "ÉC"),
            ("&", ""),
        ] {
            assert_eq!(monogram_for(name), expected, "{name}");
        }
    }

    fn sample_provider(id: &str, display_name: &str, connected: bool) -> ProviderData {
        ProviderData {
            id: id.into(),
            display_name: display_name.into(),
            connected,
            accounts: Vec::new(),
            can_add: accounts::supports_extra(id),
            api_key_mode: 0,
            api_env: String::new(),
            api_help: String::new(),
        }
    }

    #[test]
    fn connection_can_come_from_core_or_an_account() {
        let mut provider = sample_provider("codex", "Codex", false);
        assert!(!is_connected(&provider));
        provider.accounts.push((accounts::DEFAULT_KEY.into(), None, false));
        assert!(!is_connected(&provider));
        provider.connected = true;
        assert!(is_connected(&provider));
        provider.connected = false;
        provider.accounts.push(("extra".into(), None, true));
        assert!(is_connected(&provider));
    }

    #[test]
    fn visible_providers_filter_by_name_id_and_connection_and_keep_rank_order() {
        let mut claude = sample_provider("claude", "Claude", false);
        claude.accounts.push(("extra".into(), None, true));
        let providers = [
            sample_provider("q-z", "Zeta", true),
            sample_provider("openaiapi", "OpenAI API", false),
            sample_provider("q-a", "alpha", true),
            sample_provider("codex", "Codex", true),
            claude,
            sample_provider("huggingface", "Hugging Face", false),
            sample_provider("gemini", "Gemini", false),
        ];
        let ids = |query, mode| {
            visible_providers(&providers, query, mode)
                .into_iter()
                .map(|d| d.id.as_str())
                .collect::<Vec<_>>()
        };

        assert_eq!(ids("", 0), ["codex", "claude", "q-a", "q-z", "gemini", "openaiapi", "huggingface"]);
        assert_eq!(ids("", 1), ["codex", "claude", "q-a", "q-z"]);
        assert_eq!(ids("Ha", 1), ["q-a"]);
        assert_eq!(ids("OPENAI API", 0), ["openaiapi"]);
        assert_eq!(ids("oPeNaIaPi", 0), ["openaiapi"]);
        assert!(ids("Hugging", 1).is_empty());
    }
}
