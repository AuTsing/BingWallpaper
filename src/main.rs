#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use ::time::format_description;
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

fn main() {
    let log_file = Builder::new()
        .disable_cleanup(true)
        .suffix(".log")
        .tempfile()
        .unwrap();
    let (non_blocking, _guard) = non_blocking(log_file);
    let time_fmt = format_description::parse(
        "[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:3]",
    )
    .unwrap();
    let timer = LocalTime::new(time_fmt);
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_timer(timer)
        .with_target(false)
        .with_writer(non_blocking)
        .init();

    let event_loop = EventLoop::<UserEvent>::with_user_event().build().unwrap();

    let proxy = event_loop.create_proxy();
    TrayIconEvent::set_event_handler(Some(move |event| {
        proxy.send_event(UserEvent::TrayIconEvent(event)).unwrap();
    }));
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event| {
        proxy.send_event(UserEvent::MenuEvent(event)).unwrap();
    }));

    let reqwest_client = Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();

    let mut app = Application::new(event_loop.create_proxy(), reqwest_client);

    event_loop.run_app(&mut app).unwrap();
}

#[derive(Debug)]
enum UserEvent {
    TrayIconEvent(tray_icon::TrayIconEvent),
    MenuEvent(tray_icon::menu::MenuEvent),
}

struct Application {
    rt: Runtime,
    tray_icon: Option<TrayIcon>,
    menu_item_daily_update: Option<MenuItem>,
    menu_item_update: Option<MenuItem>,
    menu_item_exit: Option<MenuItem>,
    daily_updating: Option<JoinHandle<()>>,
    user_event_proxy: EventLoopProxy<UserEvent>,
    reqwest_client: Client,
    last_updated_url: Arc<Mutex<String>>,
}

impl Application {
    fn new(user_event_proxy: EventLoopProxy<UserEvent>, reqwest_client: Client) -> Application {
        Application {
            rt: Runtime::new().unwrap(),
            tray_icon: None,
            menu_item_daily_update: None,
            menu_item_update: None,
            menu_item_exit: None,
            daily_updating: None,
            user_event_proxy,
            reqwest_client,
            last_updated_url: Arc::new(Mutex::new("".to_string())),
        }
    }

    fn new_tray_icon(&mut self) -> TrayIcon {
        TrayIconBuilder::new()
            .with_menu(Box::new(Self::new_tray_menu(self)))
            .with_tooltip("BingWallpaper")
            .with_icon(Self::load_icon())
            .with_title("x")
            .build()
            .unwrap()
    }

    fn new_tray_menu(&mut self) -> Menu {
        let menu = Menu::new();

        let menu_item_daily_update = MenuItem::new("开启每日更新", true, None);
        menu.append(&menu_item_daily_update).unwrap();
        self.menu_item_daily_update = Some(menu_item_daily_update);

        let menu_item_update = MenuItem::new("更新壁纸", true, None);
        menu.append(&menu_item_update).unwrap();
        self.menu_item_update = Some(menu_item_update);

        menu.append(&PredefinedMenuItem::separator()).unwrap();

        let menu_item_exit = MenuItem::new("退出", true, None);
        menu.append(&menu_item_exit).unwrap();
        self.menu_item_exit = Some(menu_item_exit);

        menu
    }

    fn load_icon() -> Icon {
        let icon_bytes = include_bytes!("../assets/favicon.ico");
        let icon_dyn_image = image::load_from_memory(icon_bytes).unwrap();
        let rgba = icon_dyn_image.to_rgba8();
        let (width, height) = icon_dyn_image.dimensions();

        Icon::from_rgba(rgba.into_raw(), width, height).unwrap()
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
            self.tray_icon = Some(Self::new_tray_icon(self));
            let menu_event = MenuEvent {
                id: self.menu_item_daily_update.as_ref().unwrap().id().clone(),
            };
            self.user_event_proxy
                .send_event(UserEvent::MenuEvent(menu_event))
                .unwrap();
        }
    }

    fn user_event(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::TrayIconEvent(_tray_icon_event) => {}
            UserEvent::MenuEvent(menu_event) => {
                match menu_event.id {
                    _ if menu_event.id == self.menu_item_daily_update.as_ref().unwrap().id() => {
                        match self.daily_updating.as_ref() {
                            Some(handle) => {
                                handle.abort();
                                self.daily_updating = None;
                                self.menu_item_daily_update
                                    .as_ref()
                                    .unwrap()
                                    .set_text("开启每日更新");
                            }
                            None => {
                                self.daily_updating =
                                    Some(self.rt.spawn(handle_enable_daily_updating(
                                        self.reqwest_client.clone(),
                                        self.last_updated_url.clone(),
                                    )));
                                self.menu_item_daily_update
                                    .as_ref()
                                    .unwrap()
                                    .set_text("已开启每日更新");
                            }
                        }
                    }
                    _ if menu_event.id == self.menu_item_update.as_ref().unwrap().id() => {
                        self.rt.spawn(handle_update_wallpaper(
                            self.reqwest_client.clone(),
                            self.last_updated_url.clone(),
                        ));
                    }
                    _ if menu_event.id == self.menu_item_exit.as_ref().unwrap().id() => {
                        std::process::exit(0);
                    }
                    _ => {}
                };
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
    info!("开始更新壁纸");

    let latest_image_url = get_latest_image_url(&client).await.unwrap();

    if !check_needed_update(last_updated_url, &latest_image_url).await {
        return;
    }

    let latest_image_path = download_wallpaper(&client, &latest_image_url)
        .await
        .unwrap();
    set_wallpaper(&latest_image_path).unwrap();
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
