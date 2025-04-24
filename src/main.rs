#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use image::GenericImageView;
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
    let event_loop = EventLoop::<UserEvent>::with_user_event().build().unwrap();

    let proxy = event_loop.create_proxy();
    TrayIconEvent::set_event_handler(Some(move |event| {
        proxy.send_event(UserEvent::TrayIconEvent(event)).unwrap();
    }));
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event| {
        proxy.send_event(UserEvent::MenuEvent(event)).unwrap();
    }));

    let mut app = Application::new(event_loop.create_proxy());

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
    last_updated_url: Arc<Mutex<Option<String>>>,
}

impl Application {
    fn new(user_event_proxy: EventLoopProxy<UserEvent>) -> Application {
        Application {
            rt: Runtime::new().unwrap(),
            tray_icon: None,
            menu_item_daily_update: None,
            menu_item_update: None,
            menu_item_exit: None,
            daily_updating: None,
            user_event_proxy,
            last_updated_url: Arc::new(Mutex::new(None)),
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
                                let last_updated_url = Arc::clone(&self.last_updated_url);
                                self.daily_updating = Some(self.rt.spawn(async move {
                                    let mut interval = time::interval(Duration::from_secs(60 * 60));
                                    loop {
                                        interval.tick().await;
                                        handle_update_wallpaper(last_updated_url.clone())
                                            .await
                                            .unwrap();
                                    }
                                }));
                                self.menu_item_daily_update
                                    .as_ref()
                                    .unwrap()
                                    .set_text("已开启每日更新");
                            }
                        }
                    }
                    _ if menu_event.id == self.menu_item_update.as_ref().unwrap().id() => {
                        let last_updated_url = Arc::clone(&self.last_updated_url);
                        self.rt.spawn(async move {
                            handle_update_wallpaper(last_updated_url).await.unwrap();
                        });
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

async fn handle_update_wallpaper(
    last_updated_url: Arc<Mutex<Option<String>>>,
) -> Result<(), Box<dyn Error>> {
    let hp_url = "https://cn.bing.com/HPImageArchive.aspx?format=js&idx=0&n=1&mkt=zh-CN";
    let hp_response = reqwest::get(hp_url).await?;
    let hp_json = hp_response.json::<HpJson>().await?;
    let image_json = hp_json.images.get(0).ok_or("json is None")?;
    let image_url = &image_json.url;

    let mut last_updated_url = last_updated_url.lock().await;
    if last_updated_url.as_deref() == Some(image_url) {
        return Ok(());
    }
    *last_updated_url = Some(image_url.clone());
    drop(last_updated_url);

    let image_url = format!("https://s.cn.bing.net{}", &image_url);
    let image_response = reqwest::get(&image_url).await?;

    let to_file = Builder::new().keep(true).suffix(".jpg").tempfile()?;
    let mut to_file_writer = BufWriter::new(&to_file);
    let image_response_bytes = image_response.bytes().await?;
    let mut image_response_reader = image_response_bytes.as_ref();

    copy(&mut image_response_reader, &mut to_file_writer)?;

    let to_path = to_file.path().display().to_string();
    drop(to_file_writer);
    drop(to_file);

    set_wallpaper(&to_path)?;

    Ok(())
}

fn set_wallpaper(path: &str) -> Result<(), Box<dyn Error>> {
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
