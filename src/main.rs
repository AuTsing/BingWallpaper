#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use ::time::format_description;
use anyhow::Result;
use image::GenericImageView;
use reqwest::Client;
use serde::Deserialize;
use std::error::Error;
use std::ffi::OsStr;
use std::io::BufWriter;
use std::io::copy;
use std::os::windows::ffi::OsStrExt;
use std::sync::Arc;
use std::time::Duration;
use tempfile::Builder;
use tokio::runtime::Runtime;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time;
use tokio::time::MissedTickBehavior;
use tracing::error;
use tracing::info;
use tracing_appender::non_blocking;
use tracing_subscriber::fmt::time::LocalTime;
use tray_icon::Icon;
use tray_icon::TrayIcon;
use tray_icon::TrayIconBuilder;
use tray_icon::TrayIconEvent;
use tray_icon::menu::Menu;
use tray_icon::menu::MenuEvent;
use tray_icon::menu::MenuItem;
use tray_icon::menu::PredefinedMenuItem;
use windows::Win32::UI::WindowsAndMessaging::SPI_SETDESKWALLPAPER;
use windows::Win32::UI::WindowsAndMessaging::SPIF_SENDCHANGE;
use windows::Win32::UI::WindowsAndMessaging::SPIF_UPDATEINIFILE;
use windows::Win32::UI::WindowsAndMessaging::SystemParametersInfoW;
use winit::application::ApplicationHandler;
use winit::event_loop::EventLoop;
use winit::event_loop::EventLoopProxy;

fn main() -> Result<()> {
    setup_logger()?;

    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;

    let proxy = event_loop.create_proxy();
    TrayIconEvent::set_event_handler(Some(move |event| {
        if let Err(e) = proxy.send_event(UserEvent::TrayIconEvent(event)) {
            error!("Event handle error: {:?}", e);
        }
    }));
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event| {
        if let Err(e) = proxy.send_event(UserEvent::MenuEvent(event)) {
            error!("Event handle error: {:?}", e);
        }
    }));

    let proxy = event_loop.create_proxy();

    let mut app = Application::new(proxy)?;

    event_loop.run_app(&mut app)?;

    Ok(())
}

fn setup_logger() -> Result<()> {
    let log_file = Builder::new()
        .disable_cleanup(true)
        .suffix(".log")
        .tempfile()?;
    let (non_blocking, _guard) = non_blocking(log_file);
    let time_fmt = format_description::parse(
        "[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:3]",
    )?;
    let timer = LocalTime::new(time_fmt);
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_timer(timer)
        .with_target(false)
        .with_writer(non_blocking)
        .init();

    Ok(())
}

#[derive(Debug)]
enum UserEvent {
    TrayIconEvent(tray_icon::TrayIconEvent),
    MenuEvent(tray_icon::menu::MenuEvent),
}

struct Application {
    rt: Runtime,
    _tray_icon: TrayIcon,
    menu_item_daily_update: MenuItem,
    menu_item_update: MenuItem,
    menu_item_exit: MenuItem,
    daily_updating: Option<JoinHandle<()>>,
    user_event_proxy: EventLoopProxy<UserEvent>,
    reqwest_client: Client,
    last_updated_url: Arc<Mutex<String>>,
}

impl Application {
    fn new(proxy: EventLoopProxy<UserEvent>) -> Result<Self> {
        let rt = Runtime::new()?;
        let client = Client::builder()
            .pool_idle_timeout(Duration::ZERO)
            .pool_max_idle_per_host(0)
            .timeout(Duration::from_secs(3))
            .connect_timeout(Duration::from_secs(3))
            .build()?;
        let menu_item_daily_update = MenuItem::new("开启每日更新", true, None);
        let menu_item_update = MenuItem::new("更新壁纸", true, None);
        let menu_item_exit = MenuItem::new("退出", true, None);
        let tray_menu =
            Self::new_tray_menu(&menu_item_daily_update, &menu_item_update, &menu_item_exit)?;
        let tray_icon = Self::new_tray_icon(tray_menu)?;

        Ok(Self {
            rt,
            _tray_icon: tray_icon,
            menu_item_daily_update,
            menu_item_update,
            menu_item_exit,
            daily_updating: None,
            user_event_proxy: proxy,
            reqwest_client: client,
            last_updated_url: Arc::new(Mutex::new("".to_string())),
        })
    }

    fn new_tray_icon(tray_menu: Menu) -> Result<TrayIcon> {
        let icon = Self::load_icon()?;

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(tray_menu))
            .with_tooltip("BingWallpaper")
            .with_icon(icon)
            .with_title("x")
            .build()?;

        Ok(tray_icon)
    }

    fn new_tray_menu(
        menu_item_daily_update: &MenuItem,
        menu_item_update: &MenuItem,
        menu_item_exit: &MenuItem,
    ) -> Result<Menu> {
        let menu = Menu::new();

        menu.append(menu_item_daily_update)?;
        menu.append(menu_item_update)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(menu_item_exit)?;

        Ok(menu)
    }

    fn load_icon() -> Result<Icon> {
        let icon_bytes = include_bytes!("../assets/favicon.ico");
        let icon_dyn_image = image::load_from_memory(icon_bytes)?;
        let rgba = icon_dyn_image.to_rgba8();
        let (width, height) = icon_dyn_image.dimensions();

        Ok(Icon::from_rgba(rgba.into_raw(), width, height)?)
    }
}

impl ApplicationHandler<UserEvent> for Application {
    fn resumed(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) {}

    fn window_event(
        &mut self,
        _event_loop: &winit::event_loop::ActiveEventLoop,
        _window_id: winit::window::WindowId,
        _event: winit::event::WindowEvent,
    ) {
    }

    fn new_events(
        &mut self,
        _event_loop: &winit::event_loop::ActiveEventLoop,
        cause: winit::event::StartCause,
    ) {
        if winit::event::StartCause::Init == cause {
            let menu_event = MenuEvent {
                id: self.menu_item_daily_update.id().clone(),
            };
            if let Err(e) = self
                .user_event_proxy
                .send_event(UserEvent::MenuEvent(menu_event))
            {
                error!("Event handle error: {:?}", e);
            }
        }
    }

    fn user_event(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::TrayIconEvent(_tray_icon_event) => {}
            UserEvent::MenuEvent(menu_event) => {
                if menu_event.id == self.menu_item_daily_update.id() {
                    match &self.daily_updating {
                        Some(join_handle) => {
                            join_handle.abort();
                            self.daily_updating = None;
                            self.menu_item_daily_update.set_text("开启每日更新");
                        }
                        None => {
                            self.daily_updating =
                                Some(self.rt.spawn(handle_enable_daily_updating(
                                    self.reqwest_client.clone(),
                                    self.last_updated_url.clone(),
                                )));
                            self.menu_item_daily_update.set_text("已开启每日更新");
                        }
                    }
                }

                if menu_event.id == self.menu_item_update.id() {
                    self.rt.spawn(handle_update_wallpaper(
                        self.reqwest_client.clone(),
                        self.last_updated_url.clone(),
                    ));
                }

                if menu_event.id == self.menu_item_exit.id() {
                    std::process::exit(0);
                }
            }
        };
    }
}

#[derive(Deserialize)]
struct HpImage {
    url: String,
}

#[derive(Deserialize)]
struct HpJson {
    images: Vec<HpImage>,
}

async fn handle_enable_daily_updating(client: Client, last_updated_url: Arc<Mutex<String>>) {
    let mut interval = time::interval(Duration::from_secs(60 * 60));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        handle_update_wallpaper(client.clone(), last_updated_url.clone()).await;
    }
}

async fn handle_update_wallpaper(client: Client, last_updated_url: Arc<Mutex<String>>) {
    if let Err(e) = update_wallpaper(client, last_updated_url).await {
        error!("更新壁纸失败: {:?}", e);
    };
}

async fn update_wallpaper(
    client: Client,
    last_updated_url: Arc<Mutex<String>>,
) -> Result<(), Box<dyn Error>> {
    info!("开始更新壁纸");

    let latest_image_url = get_latest_image_url(&client).await?;

    if !check_needed_update(last_updated_url, &latest_image_url).await {
        return Ok(());
    }

    let latest_image_path = download_wallpaper(&client, &latest_image_url).await?;

    set_wallpaper(&latest_image_path)?;

    Ok(())
}

async fn get_latest_image_url(client: &Client) -> Result<String, Box<dyn Error>> {
    let hp_url = "https://cn.bing.com/HPImageArchive.aspx?format=js&idx=0&n=1&mkt=zh-CN";
    let hp_response = client.get(hp_url).send().await?;
    let hp_json = hp_response.json::<HpJson>().await?;
    let image_json = hp_json.images.get(0).ok_or("json is None")?;
    let image_url = &image_json.url;

    info!("更新链接: {}", image_url);

    Ok(image_url.clone())
}

async fn check_needed_update(
    last_updated_url: Arc<Mutex<String>>,
    latest_image_url: &String,
) -> bool {
    let mut last_updated_url = last_updated_url.lock().await;
    if &*last_updated_url == latest_image_url {
        info!("更新链接相同，跳过更新");
        return false;
    }

    *last_updated_url = latest_image_url.clone();
    true
}

async fn download_wallpaper(
    client: &Client,
    latest_image_url: &String,
) -> Result<String, Box<dyn Error>> {
    info!("下载壁纸");

    let image_url = format!("https://s.cn.bing.net{}", latest_image_url);
    let image_response = client.get(&image_url).send().await?;

    let to_file = Builder::new()
        .disable_cleanup(true)
        .suffix(".jpg")
        .tempfile()?;
    let mut to_file_writer = BufWriter::new(&to_file);
    let image_response_bytes = image_response.bytes().await?;
    let mut image_response_reader = image_response_bytes.as_ref();

    copy(&mut image_response_reader, &mut to_file_writer)?;

    let to_path = to_file.path().display().to_string();

    info!("保存壁纸: {}", &to_path);

    Ok(to_path)
}

fn set_wallpaper(path: &String) -> Result<(), Box<dyn Error>> {
    info!("应用壁纸");

    let wide: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        SystemParametersInfoW(
            SPI_SETDESKWALLPAPER,
            0,
            Some(wide.as_ptr() as _),
            SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
        )?;
    }

    Ok(())
}
