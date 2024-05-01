// SPDX-License-Identifier: GPL-3.0-or-later

use crate::backend::pwprofileobject::PwProfileObject;
use glib::{
    self, clone,
    subclass::{prelude::*, Signal},
    Object, ObjectExt, ParamSpec, Properties, Value,
};
use gtk::{gio, prelude::*};
use wireplumber as wp;
use wp::{
    pw::{PipewireObjectExt, PipewireObjectExt2},
    spa::SpaPodBuilder,
};

use crate::macros::*;
use once_cell::sync::{Lazy, OnceCell};
use super::{PwRouteFilterModel, PwRouteObject, RouteDirection};
use std::cell::{Cell, RefCell};

pub mod imp {

    use super::*;

    #[derive(Properties)]
    #[properties(wrapper_type = super::PwDeviceObject)]
    pub struct PwDeviceObject {
        #[property(get, set)]
        name: RefCell<Option<String>>,

        #[property(get, set)]
        icon_name: RefCell<String>,

        #[property(get, set)]
        pub(super) profile_index: Cell<u32>,

        #[property(get, set)]
        pub(super) route_index_input: Cell<u32>,

        #[property(get, set)]
        pub(super) route_index_output: Cell<u32>,

        #[property(get, set, construct_only)]
        pub(super) wpdevice: OnceCell<wp::pw::Device>,

        #[property(get)]
        pub(super) profilemodel: gio::ListStore,

        #[property(get)]
        pub(super) routemodel_input: PwRouteFilterModel,

        #[property(get)]
        pub(super) routemodel_output: PwRouteFilterModel,

        pub(super) routemodel: gio::ListStore,
    }

    impl Default for PwDeviceObject {
        fn default() -> Self {
            Self {
                name: Default::default(),
                icon_name: Default::default(),
                profile_index: Default::default(),
                route_index_input: Default::default(),
                route_index_output: Default::default(),
                wpdevice: Default::default(),
                profilemodel: gio::ListStore::new::<PwProfileObject>(),
                routemodel_input: PwRouteFilterModel::new(RouteDirection::Input, gio::ListModel::NONE),
                routemodel_output: PwRouteFilterModel::new(RouteDirection::Output, gio::ListModel::NONE),
                routemodel: gio::ListStore::new::<PwRouteObject>(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PwDeviceObject {
        const NAME: &'static str = "PwDeviceObject";
        type Type = super::PwDeviceObject;
    }

    // Trait shared by all GObjects
    impl ObjectImpl for PwDeviceObject {
        fn properties() -> &'static [ParamSpec] {
            Self::derived_properties()
        }

        fn set_property(&self, id: usize, value: &Value, pspec: &ParamSpec) {
            self.derived_set_property(id, value, pspec);
        }

        fn property(&self, id: usize, pspec: &ParamSpec) -> Value {
            self.derived_property(id, pspec)
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: Lazy<Vec<Signal>> = Lazy::new(|| {
                vec![
                    Signal::builder("pre-update-profile").build(),
                    Signal::builder("post-update-profile").build(),
                    Signal::builder("pre-update-route").build(),
                    Signal::builder("post-update-route").build(),
                ]
            });

            SIGNALS.as_ref()
        }

        fn constructed(&self) {
            self.parent_constructed();

            self.routemodel_input.set_model(Some(self.routemodel.as_ref()));
            self.routemodel_output.set_model(Some(self.routemodel.as_ref()));

            let obj = self.obj();

            obj.label_set_name();
            obj.update_icon_name();
            obj.update_profiles();

            if let Some(index) = obj.get_current_profile_index() {
                obj.set_profile_index(index as u32);
            }

            obj.update_routes();

            obj.wpdevice().connect_properties_notify(clone!(@weak obj => move |device| {
                pwvucontrol_debug!("properties changed! id: {}", device.object_id().unwrap());

                obj.label_set_name();
            }));

            obj.wpdevice().connect_params_changed(clone!(@weak obj => move |device, what| {
                pwvucontrol_debug!("params-changed! {what} id: {}", device.object_id().unwrap());

                match what {
                    "EnumProfile" => {
                        obj.update_profiles();
                        //obj.emit_by_name::<()>("profiles-changed", &[]);
                    },
                    "Profile" => {
                        if let Some(index) = obj.get_current_profile_index() {
                            obj.set_profile_index(index as u32);
                        }
                    },
                    "EnumRoute" => {
                        obj.update_routes();
                    },
                    "Route" => {
                        //obj.update_routes();
                        obj.update_current_route_index();
                    },
                    _ => {},
                }

            }));
        }
    }

    impl PwDeviceObject {}
}

glib::wrapper! {
    pub struct PwDeviceObject(ObjectSubclass<imp::PwDeviceObject>);
}

impl PwDeviceObject {
    pub(crate) fn new(node: &wp::pw::Device) -> Self {
        Object::builder().property("wpdevice", node).build()
    }

    pub(crate) fn update_profiles(&self) {
        let device = self.wpdevice();

        device.enum_params(
            Some("EnumProfile"),
            None,
            gtk::gio::Cancellable::NONE,
            clone!(@weak self as widget => move |res| {
                let keys = wp::spa::SpaIdTable::from_name("Spa:Pod:Object:Param:Profile").expect("id table");
                let index_key = keys.find_value_from_short_name("index").expect("index key");
                let description_key = keys.find_value_from_short_name("description").expect("decription key");
                let available_key = keys.find_value_from_short_name("available").expect("available key");

                if let Ok(Some(iter)) = res {
                    let removed = widget.profilemodel().n_items();

                    widget.emit_by_name::<()>("pre-update-profile", &[]);

                    let mut profiles: Vec<PwProfileObject> = Vec::new();

                    for a in iter {
                        let pod: wp::spa::SpaPod = a.get().unwrap();
                        if !pod.is_object() {
                            continue;
                        }

                        let index = pod.find_spa_property(&index_key).expect("Index!").int().expect("Int");
                        let description = pod.find_spa_property(&description_key).expect("Format!").string().expect("String");
                        let available = pod.find_spa_property(&available_key).expect("Availability!").id().expect("Id");

                        profiles.push(PwProfileObject::new(index as u32, &description, available));
                    }
                    widget.profilemodel().splice(0, removed as u32, &profiles);

                    // Set profile_index property without notify by setting internal storage directly
                    widget.imp().profile_index.set(widget.get_current_profile_index().unwrap() as u32);

                    // Notify update of list model
                    widget.emit_by_name::<()>("post-update-profile", &[]);
                } else {
                    if let Err(e) = res {
                        dbg!(e);
                    }
                }
            }),
        );
    }

    pub(crate) fn get_current_profile_index(&self) -> Option<i32> {
        let device = self.wpdevice();

        let keys = wp::spa::SpaIdTable::from_name("Spa:Pod:Object:Param:Profile").expect("id table");
        let index_key = keys.find_value_from_short_name("index").expect("index key");
        let description_key = keys.find_value_from_short_name("description").expect("decription key");

        if let Some(params) = device.enum_params_sync("Profile", None) {
            for a in params {
                let pod: wp::spa::SpaPod = a.get().unwrap();
                if !pod.is_object() {
                    continue;
                }

                let index = pod.find_spa_property(&index_key).expect("Index!").int().expect("Int");
                let description = pod.find_spa_property(&description_key).expect("Format!").string().expect("String");
                pwvucontrol_info!("Current profile #{} {}", index, description);

                return Some(index);
            }
        }

        None
    }

    pub(crate) fn set_profile(&self, index: i32) {
        let device = self.wpdevice();

        let podbuilder = SpaPodBuilder::new_object("Spa:Pod:Object:Param:Profile", "Profile");

        podbuilder.add_property("index");
        podbuilder.add_int(index);

        if let Some(pod) = podbuilder.end() {
            device.set_param("Profile", 0, pod);
        }
    }

    pub(crate) fn update_routes(&self) {
        let device = self.wpdevice();

        device.enum_params(
            Some("EnumRoute"),
            None,
            gtk::gio::Cancellable::NONE,
            clone!(@weak self as widget => move |res| {
                let keys = wp::spa::SpaIdTable::from_name("Spa:Pod:Object:Param:Route").expect("id table");
                let index_key = keys.find_value_from_short_name("index").expect("index key");
                let description_key = keys.find_value_from_short_name("description").expect("decription key");
                let available_key = keys.find_value_from_short_name("available").expect("available key");
                let direction_key = keys.find_value_from_short_name("direction").expect("direction key");

                if let Ok(Some(iter)) = res {
                    let removed = widget.imp().routemodel.n_items();
                    widget.emit_by_name::<()>("pre-update-route", &[]);

                    let mut routes: Vec<PwRouteObject> = Vec::new();

                    for a in iter {
                        let pod: wp::spa::SpaPod = a.get().unwrap();
                        if !pod.is_object() {
                            continue;
                        }

                        let index = pod.find_spa_property(&index_key).expect("Index").int().expect("Int");
                        let description = pod.find_spa_property(&description_key).expect("Format!").string().expect("String");
                        let available = pod.find_spa_property(&available_key).expect("Availability!").id().expect("Id");
                        let direction = pod.find_spa_property(&direction_key).expect("Direction!").id().expect("Id");

                        routes.push(PwRouteObject::new(index as u32, &description, available, direction));
                    }
                    widget.imp().routemodel.splice(0, removed as u32, &routes);

                    // Set route_index property without notify by setting internal storage directly
                    widget.update_current_route_index();

                    // Notify update of list model
                    widget.emit_by_name::<()>("post-update-route", &[]);
                } else {
                    if let Err(e) = res {
                        dbg!(e);
                    }
                }
            }),
        );
    }

    pub(crate) fn update_current_route_index(&self) {
        self.update_current_route_index_for_direction_sync(RouteDirection::Input);
        self.update_current_route_index_for_direction_sync(RouteDirection::Output);
    }

    pub(crate) fn update_current_route_index_for_direction_sync(&self, direction: RouteDirection) {
        let device = self.wpdevice();

        let keys = wp::spa::SpaIdTable::from_name("Spa:Pod:Object:Param:Route").expect("id table");
        let index_key = keys.find_value_from_short_name("index").expect("index key");
        let description_key = keys.find_value_from_short_name("description").expect("decription key");

        let podbuilder = SpaPodBuilder::new_object("Spa:Pod:Object:Param:Route", "Route");
        podbuilder.add_property("direction");
        podbuilder.add_id(direction.into());
        let filter_pod = podbuilder.end().expect("pod");

        if let Some(params) = device.enum_params_sync("Route", Some(&filter_pod)) {
            for a in params {
                let pod: wp::spa::SpaPod = a.get().unwrap();
                if !pod.is_object() {
                    continue;
                }

                let index = pod.find_spa_property(&index_key).expect("Index").int().expect("Int");
                let description = pod
                    .find_spa_property(&description_key)
                    .expect("Description key")
                    .string()
                    .expect("String");

                pwvucontrol_info!("Current route #{} {}", index, description);

                if let Some(modelindex) = self.get_model_index_from_route_index(direction, index) {
                    match direction {
                        RouteDirection::Input => self.set_route_index_input(modelindex),
                        RouteDirection::Output => self.set_route_index_output(modelindex),
                        _ => unreachable!()
                    }
                //     match direction {
                //         RouteDirection::Input => self.imp().route_index_input.set(modelindex),
                //         RouteDirection::Output => self.imp().route_index_output.set(modelindex),
                //         _ => unreachable!()
                //     }
                //     self.emit_by_name::<()>("post-update-route", &[]);
                // } else {
                    pwvucontrol_critical!("{direction:?} Unable to get model index from route index in update_current_route_index_for_direction_sync");
                };
            }
        }
    }

    fn get_model_index_from_route_index(&self, direction: RouteDirection, routeindex: i32) -> Option<u32> {
        let routemodel = self.get_route_model_for_direction(direction);

        pwvucontrol_info!("{direction:?} routemodel.n_items = {}, routeindex = {routeindex}", routemodel.n_items());
        for (i, x) in routemodel.iter::<PwRouteObject>().map_while(Result::ok).enumerate() {
            pwvucontrol_info!("{direction:?} #{i} item.index = {}, item.description = {}", x.index(), x.description());
        }

        for (i, o) in routemodel.iter::<PwRouteObject>().enumerate() {
            if let Ok(obj) = o {
                //let b: PwRouteObject = b.downcast().expect("PwRouteObject");
                if obj.index() as u32 == routeindex as u32 {
                    return Some(i as u32);
                }
            } else {
                pwvucontrol_critical!("model mutated while iterating, returning None");
            }
        }
        None
    }

    pub(crate) fn set_route(&self, index: u32, device_index: i32) {
        let device = self.wpdevice();

        let podbuilder = SpaPodBuilder::new_object("Spa:Pod:Object:Param:Route", "Route");

        podbuilder.add_property("index");
        podbuilder.add_int(index as i32);
        podbuilder.add_property("device");
        podbuilder.add_int(device_index);
        // podbuilder.add_property("save");
        // podbuilder.add_boolean(true);

        if let Some(pod) = podbuilder.end() {
            device.set_param("Route", 0, pod);
        }
    }

    fn get_route_model_for_direction(&self, direction: RouteDirection) -> PwRouteFilterModel {
        match direction {
            RouteDirection::Input => self.routemodel_input(),
            RouteDirection::Output => self.routemodel_output(),
            _ => unreachable!(),
        }
    }

    fn label_set_name(&self) {
        let description: String = self.wpdevice().pw_property("device.description").expect("device description");
        self.set_name(description);
    }

    fn update_icon_name(&self) {
        let icon_name: String = self
            .wpdevice()
            .pw_property("device.icon-name")
            .unwrap_or("soundcard-symbolic".to_string());
        self.set_icon_name(icon_name);
    }
}
