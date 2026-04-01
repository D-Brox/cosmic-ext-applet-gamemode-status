// SPDX-License-Identifier: GPL-3.0-only

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::Path;

use cosmic::app::{Core, Task};
use cosmic::cosmic_theme::Layer;
use cosmic::iced::platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup};
use cosmic::iced::window::Id;
use cosmic::iced::{Alignment, Length, Subscription, stream};
use cosmic::widget::{Column, Grid, JustifyContent, Text, layer_container};
use cosmic::{Application, Element};

use crate::dbus::GameModeProxy;
use futures_util::SinkExt;
use futures_util::stream::StreamExt;
use sysinfo::{
    Pid, Process, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System, UpdateKind,
};
use zbus::Connection;

use crate::fl;

#[derive(Default)]
pub struct GameModeStatus {
    core: Core,
    sys: System,
    games: HashMap<usize, String>,
    popup: Option<Id>,
}

#[derive(Debug, Clone)]
pub enum Message {
    TogglePopup,
    PopupClosed(Id),
    GameListAdd(usize),
    GameListRemove(usize),
    GameListSet(Vec<usize>),
}

impl Application for GameModeStatus {
    type Executor = cosmic::executor::Default;

    type Flags = ();

    type Message = Message;

    const APP_ID: &'static str = "dev.DBrox.CosmicGameModeStatus";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: Self::Flags) -> (Self, Task<Self::Message>) {
        let sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_processes(ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet)),
        );
        let app = GameModeStatus {
            core,
            sys,
            ..Default::default()
        };

        (app, Self::init_game_list())
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn view(&self) -> Element<'_, Self::Message> {
        self.core
            .applet
            .icon_button(if self.games.is_empty() {
                "display-symbolic"
            } else {
                "applications-games-symbolic"
            })
            .on_press(Message::TogglePopup)
            .into()
    }

    fn view_window(&self, _id: Id) -> Element<'_, Self::Message> {
        self.core
            .applet
            .popup_container(
                Column::new()
                    .align_x(Alignment::Center)
                    .push(
                        Text::new(if self.games.is_empty() {
                            fl!("gamemode-off")
                        } else {
                            fl!("gamemode-on")
                        })
                        .align_x(Alignment::Center),
                    )
                    .push(
                        layer_container(if self.games.is_empty() {
                            Text::new(fl!("no-active-clients"))
                                .align_x(Alignment::Center)
                                .into()
                        } else {
                            self.game_grid()
                        })
                        .layer(Layer::Primary)
                        .padding(10),
                    )
                    .padding(10)
                    .spacing(5),
            )
            .into()
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::TogglePopup => {
                return if let Some(p) = self.popup.take() {
                    destroy_popup(p)
                } else {
                    let new_id = Id::unique();
                    self.popup.replace(new_id);
                    let popup_settings = self.core.applet.get_popup_settings(
                        self.core.main_window_id().unwrap(),
                        new_id,
                        None,
                        None,
                        None,
                    );
                    get_popup(popup_settings)
                };
            }
            Message::PopupClosed(id) => {
                if self.popup.as_ref() == Some(&id) {
                    self.popup = None;
                }
            }
            Message::GameListAdd(pid) => {
                let p = Pid::from(pid);
                self.sys
                    .refresh_processes(ProcessesToUpdate::Some(&[p]), true);
                if let Some(exe_str) = self
                    .sys
                    .process(p)
                    .and_then(Process::exe)
                    .and_then(Path::file_name)
                    .and_then(OsStr::to_str)
                {
                    let exe = exe_str.to_string();
                    self.games.insert(pid, exe);
                }
            }
            Message::GameListRemove(pid) => {
                self.games.remove(&pid);
            }
            Message::GameListSet(list) => {
                self.games = HashMap::new();
                self.sys.refresh_processes(
                    ProcessesToUpdate::Some(
                        &list.iter().map(|pid| Pid::from(*pid)).collect::<Vec<_>>(),
                    ),
                    true,
                );
                for pid in &list {
                    if let Some(exe_str) = self
                        .sys
                        .process(Pid::from(*pid))
                        .and_then(Process::exe)
                        .and_then(Path::file_name)
                        .and_then(OsStr::to_str)
                    {
                        let exe = exe_str.to_string();
                        self.games.insert(*pid, exe);
                    }
                }
            }
        }
        Task::none()
    }

    fn subscription(&self) -> cosmic::iced::Subscription<Self::Message> {
        let registered = Subscription::run(move || {
            stream::channel(100, async |mut output| {
                let conn = Connection::session()
                    .await
                    .expect("Failed to start dbus session");
                let proxy = GameModeProxy::new(&conn)
                    .await
                    .expect("Failed to get proxy");
                let mut registered = proxy
                    .receive_game_registered()
                    .await
                    .expect("Failed to get GameRegistered signal");

                #[allow(clippy::cast_sign_loss)]
                while let Some(msg) = registered.next().await {
                    let args = msg.args().expect("failed to get args");
                    _ = output.send(Message::GameListAdd(args.pid as usize)).await;
                }
                panic!("Stream ended unexpectedly");
            })
        });
        let unregistered = Subscription::run(move || {
            stream::channel(100, async |mut output| {
                let conn = Connection::session()
                    .await
                    .expect("Failed to start dbus session");
                let proxy = GameModeProxy::new(&conn)
                    .await
                    .expect("Failed to get proxy");
                let mut unregistered = proxy
                    .receive_game_unregistered()
                    .await
                    .expect("Failed to get GameRegistered signal");

                #[allow(clippy::cast_sign_loss)]
                while let Some(msg) = unregistered.next().await {
                    let args = msg.args().expect("failed to get args");
                    _ = output
                        .send(Message::GameListRemove(args.pid as usize))
                        .await;
                }
                panic!("Stream ended unexpectedly");
            })
        });

        Subscription::batch(vec![registered, unregistered])
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }
}

impl GameModeStatus {
    fn game_grid(&self) -> Element<'_, Message> {
        let mut grid = Grid::<Message>::new()
            .push(Text::new("PID"))
            .push(Text::new(fl!("name")));

        for (pid, name) in &self.games {
            grid = grid
                .insert_row()
                .push(Text::new(pid.to_string()))
                .push(Text::new(name));
        }

        grid.column_alignment(Alignment::Center)
            .row_alignment(Alignment::Center)
            .height(Length::Shrink)
            .width(Length::Shrink)
            .column_spacing(20)
            .justify_content(JustifyContent::SpaceEvenly)
            .into()
    }

    fn init_game_list() -> Task<Message> {
        Task::perform(
            #[allow(clippy::cast_sign_loss)]
            async {
                let conn = Connection::session()
                    .await
                    .expect("Failed to start dbus session");
                let proxy = GameModeProxy::new(&conn)
                    .await
                    .expect("Failed to get proxy");
                let list = proxy.list_games().await.expect("Failed to get list");
                list.iter().map(|g| g.0 as usize).collect::<Vec<_>>()
            },
            |res| cosmic::Action::App(Message::GameListSet(res)),
        )
    }
}
