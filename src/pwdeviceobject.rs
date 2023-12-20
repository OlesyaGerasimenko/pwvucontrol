// SPDX-License-Identifier: GPL-3.0-or-later

use glib::{self, clone, subclass::prelude::*, Object, ObjectExt};

use gtk::{gio, prelude::ListModelExt};
use wireplumber as wp;
use wp::{
    pw::{PipewireObjectExt, PipewireObjectExt2},
    spa::SpaPodBuilder,
};

//use im_rc::HashMap;
use std::collections::HashMap;

pub mod imp {
    use super::*;

    use glib::{ParamSpec, Properties, Value, subclass::Signal};
    use gtk::{gio, glib, prelude::*, subclass::prelude::*};
    use once_cell::sync::{OnceCell, Lazy};
    use std::cell::{Cell, RefCell};

    #[derive(Default, Properties)]
    #[properties(wrapper_type = super::PwDeviceObject)]
    pub struct PwDeviceObject {
        #[property(get, set)]
        name: RefCell<Option<String>>,
        #[property(get, set)]
        iconname: RefCell<String>,
        #[property(get, set)]
        pub(super) profile_index: Cell<u32>,

        #[property(get, set, construct_only)]
        pub(super) wpdevice: OnceCell<wp::pw::Device>,

        pub(super) profiles: RefCell<HashMap<u32, String>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PwDeviceObject {
        const NAME: &'static str = "PwDeviceObject";
        type Type = super::PwDeviceObject;
        type Interfaces = (gio::ListModel,);
    }

    impl ListModelImpl for PwDeviceObject {
        fn item_type(&self) -> glib::Type {
            gtk::StringObject::static_type()
        }

        fn n_items(&self) -> u32 {
            self.profiles.borrow().len() as u32
        }

        fn item(&self, position: u32) -> Option<glib::Object> {
            match self.profiles.borrow().get(&position) {
                Some(item) => {
                    let stringobj = gtk::StringObject::new(item);
                    Some(stringobj.upcast::<glib::Object>())
                }
                None => None,
            }
        }
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
            static SIGNALS: Lazy<Vec<Signal>> =
                Lazy::new(|| vec![
                    Signal::builder("profiles-changed")
                    //.param_types([i32::static_type()])
                    .build(),
                    Signal::builder("pre-update").build(),
                    Signal::builder("post-update").build(),
                ]);

            SIGNALS.as_ref()
        }

        fn constructed(&self) {
            self.parent_constructed();

            let obj = self.obj();

            obj.label_set_name();
            obj.update_icon_name();
            obj.update_profiles();

            if let Some(index) = obj.get_current_profile_index() {
                obj.set_profile_index(index as u32);
            }

            obj.wpdevice()
                .connect_properties_notify(clone!(@weak obj => move |device| {
                    wp::log::debug!("properties changed! id: {}", device.object_id().unwrap());

                    obj.label_set_name();
                }));

            obj.wpdevice()
                .connect_params_changed(clone!(@weak obj => move |device, what| {
                    wp::log::debug!("params-changed! {what} id: {}", device.object_id().unwrap());

                    match what {
                        "EnumProfile" => {
                            obj.update_profiles();
                            //obj.emit_by_name::<()>("profiles-changed", &[]);
                        },
                        "Profile" => {
                            if let Some(index) = obj.get_current_profile_index() {
                                obj.set_profile_index(index as u32);
                            }
                        }
                        _ => {},
                    }

                }));

        }
    }

    impl PwDeviceObject {}
}

glib::wrapper! {
    pub struct PwDeviceObject(ObjectSubclass<imp::PwDeviceObject>) @implements gio::ListModel;
}

impl PwDeviceObject {
    pub(crate) fn new(node: &wp::pw::Device) -> Self {
        Object::builder().property("wpdevice", node).build()
    }

    pub(crate) fn update_profiles(&self) {
        let device = self.wpdevice();
        let deviceid = device.object_id().expect("device id");

        let infomsg = format!(
            "Listing profiles for device #{deviceid} {}",
            device
                .pw_property::<String>("device.nick")
                .expect("device.nick")
        );

        device.enum_params(Some("EnumProfile"), None, gtk::gio::Cancellable::NONE, 
            clone!(@weak self as widget => move |res| {
                let keys = wp::spa::SpaIdTable::from_name("Spa:Pod:Object:Param:Profile").expect("id table");
                let index_key = keys.find_value_from_short_name("index").expect("index key");
                let description_key = keys.find_value_from_short_name("description").expect("decription key");

                if let Ok(Some(iter)) = res {

                    wp::log::info!("{infomsg}");

                    let removed = widget.imp().profiles.borrow().len();

                    let inserted = {
                        let mut profiles = widget.imp().profiles.borrow_mut();
                        profiles.clear();

                        for a in iter {
                            let pod: wp::spa::SpaPod = a.get().unwrap();
                            if !pod.is_object() {
                                continue;
                            }

                            let index = pod.find_spa_property(&index_key).expect("Index!").int().expect("Int");
                            let description = pod.find_spa_property(&description_key).expect("Format!").string().expect("String");

                            profiles.insert(index as u32, description.to_string());

                            wp::log::info!("Got profile #{} {}", index, description);
                        }

                        profiles.len()
                    };

                    // Set profile_index property without notify by setting internal storage directly
                    widget.imp().profile_index.set(widget.get_current_profile_index().unwrap() as u32);

                    // Notify update of list model
                    widget.emit_by_name::<()>("pre-update", &[]);
                    widget.items_changed(0, removed as u32, inserted as u32);
                    widget.emit_by_name::<()>("post-update", &[]);
                    
                    //widget.emit_by_name::<()>("profiles-changed", &[]);
                } else {
                    if let Err(e) = res {
                        dbg!(e);
                    }
                }
        }));
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

                wp::log::info!("Current profile #{} {}", index, description);

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

    pub(crate) fn get_profiles(&self) -> HashMap<u32, String> {
        self.imp().profiles.borrow().clone()
    }

    fn label_set_name(&self) {
        let description: String = self
            .wpdevice()
            .pw_property("device.description")
            .expect("device description");
        self.set_name(description);
    }

    fn update_icon_name(&self) {
        self.set_iconname("soundcard-symbolic");
    }

    // pub(crate) fn serial(&self) -> u32 {
    //     let serial: i32 = self
    //         .wpdevice()
    //         .pw_property("object.serial")
    //         .expect("object.serial");

    //     serial as u32
    // }

    // pub(crate) fn device_property<T: FromPipewirePropertyString>(&self, property: &str) -> T {
    //     self.wpdevice()
    //         .pw_property(property)
    //         .expect("object.serial")
    // }
}
