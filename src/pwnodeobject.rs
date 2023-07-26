use glib::{self, clone, subclass::prelude::*, Object, ObjectExt, Cast};

use wireplumber as wp;
use wp::{pw::{GlobalProxyExt, PipewireObjectExt, PipewireObjectExt2, ProxyExt, MetadataExt}, spa::SpaPodBuilder, registry::{Constraint, ConstraintType, Interest}};

use crate::{NodeType, application::PwvucontrolApplication, window::PwvucontrolWindow};

mod mixerapi;
#[derive(Copy, Clone, Debug)]
pub struct AudioFormat {
    pub channels: i32,
    pub format: u32,
    pub rate: i32,
    pub positions: [u32; 64],
}

pub(crate) enum PropertyChanged {
    Volume,
    Mute,
    ChannelVolumes
}

pub mod imp {
    use super::*;

    use std::cell::{Cell, RefCell};
    use glib::{
        ParamSpec,
        Properties,
        Value,
        subclass::Signal
    };
    use once_cell::sync::{Lazy, OnceCell};

    // Object holding the state
    #[derive(Default, Properties)]
    #[properties(wrapper_type = super::PwNodeObject)]
    pub struct PwNodeObject {
        #[property(get, set)]
        name: RefCell<Option<String>>,
        #[property(get, set)]
        description: RefCell<Option<String>>,
        #[property(get, set)]
        formatstr: RefCell<Option<String>>,
        #[property(get, set)]
        boundid: Cell<u32>,
        #[property(get, set)]
        mainvolume: Cell<f32>,
        #[property(get, set)]
        volume: Cell<f32>,
        #[property(get, set)]
        mute: Cell<bool>,
        #[property(get = Self::channel_volumes, set = Self::set_channel_volumes, type = glib::ValueArray)]
        pub(super) channel_volumes: RefCell<Vec<f32>>,
        #[property(get, set, builder(crate::NodeType::Undefined))]
        nodetype: Cell<crate::NodeType>,

        pub(super) format: Cell<Option<AudioFormat>>,

        #[property(get, set)]
        pub(super) channellock: Cell<bool>,

        #[property(get, set, construct_only)]
        pub(super) wpnode: OnceCell<wp::pw::Node>,

        pub(super) mixerapi: OnceCell<wp::plugin::Plugin>,

        pub(super) block: Cell<bool>,
    }

    // The central trait for subclassing a GObject
    #[glib::object_subclass]
    impl ObjectSubclass for PwNodeObject {
        const NAME: &'static str = "PwNodeObject";
        type Type = super::PwNodeObject;
    }

    // Trait shared by all GObjects
    impl ObjectImpl for PwNodeObject {
        fn properties() -> &'static [ParamSpec] {
            Self::derived_properties()
        }

        fn set_property(&self, id: usize, value: &Value, pspec: &ParamSpec) {
            self.derived_set_property(id, value, pspec);
            match pspec.name() {
                "volume" => {
                    if self.block.get() == false {
                        // self.obj().send_volume(PropertyChanged::Volume);
                        self.obj().send_volume_using_mixerapi(PropertyChanged::Volume);
                    }
                },
                "mute" => {
                    if self.block.get() == false {
                        // self.obj().send_volume(PropertyChanged::Mute);
                        self.obj().send_volume_using_mixerapi(PropertyChanged::Mute);
                    }
                },
                "mainvolume" => {
                    if self.block.get() == false {
                        self.obj().send_mainvolume();
                    }
                },
                _ => {},
            }
        }

        fn notify(&self, pspec: &ParamSpec) {
            if pspec.name() == "channel-volumes" {
                if self.block.get() == false {
                    // self.obj().send_volume(PropertyChanged::ChannelVolumes);
                    self.obj().send_volume_using_mixerapi(PropertyChanged::ChannelVolumes);
                }
            }
        }

        fn property(&self, id: usize, pspec: &ParamSpec) -> Value {
            self.derived_property(id, pspec)
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: Lazy<Vec<Signal>> =
                Lazy::new(|| vec![Signal::builder("format").build()]);

            SIGNALS.as_ref()
        }


        fn constructed(&self) {
            let obj = self.obj();

            let node = self.wpnode.get().expect("Node set on PwNodeObject");

            node.connect_notify_local(
                Some("global-properties"),
                clone!(@weak obj => move  |_, _| {
                    obj.label_set_name()
                }),
            );

            node.connect_notify_local(
                Some("properties"),
                clone!(@weak obj => move  |_, _| {
                    obj.label_set_description();
                }),
            );

            node.connect_params_changed(clone!(@weak obj => move |_node,what| {
                wp::log::info!("params-changed! {what}");
                obj.imp().block.set(true);
                match what {
                    "Props" => obj.update_mainvolume(),
                    "Format" => obj.update_format(),
                    _ => {},
                }
                obj.imp().block.set(false);
            }));

            obj.label_set_description();
            obj.update_mainvolume();
            obj.update_format();
            obj.label_set_name();

            obj.get_mixer_api();
            obj.update_volume_using_mixerapi();

        }
    }

    impl PwNodeObject {
        pub fn channel_volumes(&self) -> glib::ValueArray {
            let mut values = glib::ValueArray::new(self.channel_volumes.borrow().len() as u32);
            let channel_volumes = self.channel_volumes.borrow();
            channel_volumes.iter().for_each(|volume| {
                values.append(&Value::from(volume));
            });

            values
        }

        pub fn set_channel_volumes(&self, values: glib::ValueArray) {
            let mut channel_volumes = self.channel_volumes.borrow_mut();
            values.iter().for_each(|value| {
                if let Ok(volume) = value.get() {
                    channel_volumes.push(volume);
                }
            });
        }
    }
}

glib::wrapper! {
    pub struct PwNodeObject(ObjectSubclass<imp::PwNodeObject>);
}

impl PwNodeObject {
    pub(crate) fn new(node: &wp::pw::Node) -> Self {
        let nodetype = match node.get_pw_property("media.class").as_deref() {
            Some("Stream/Output/Audio") => NodeType::Output,
            Some("Stream/Input/Audio") => NodeType::Input,
            Some("Audio/Sink") => NodeType::Sink,
            _ => NodeType::Undefined,
        };

        Object::builder()
            .property("boundid", node.bound_id())
            .property("wpnode", node)
            .property("nodetype", nodetype)
            .build()
    }

    fn label_set_name(&self) {
        let wp_node = self
            .imp()
            .wpnode
            .get()
            .expect("Node widget should always have a wp_node");
        let props = wp_node.global_properties().expect("Node has no properties");

        let name_gstr = match self.nodetype() {
            NodeType::Sink => {
                props
                .get("node.description")
                .or_else(|| props.get("node.nick"))
                .or_else(|| props.get("node.name"))
            },
            _ => {
                props
                .get("node.nick")
                .or_else(|| props.get("node.description"))
                .or_else(|| props.get("node.name"))
            }
        };

        let name = name_gstr
            .as_ref()
            .map(|name| name.as_str())
            .unwrap_or_default();

        self.set_name(name);
    }

    fn label_set_description(&self) {
        let wp_node = self
            .imp()
            .wpnode
            .get()
            .expect("Node widget should always have a wp_node");
        let props = wp_node.properties().expect("Node has no properties");
        let name_gstr = props
            .get("media.name");

        let name = name_gstr
            .as_ref()
            .map(|name| name.as_str())
            .unwrap_or_default();

        self.set_description(name);
    }

    pub(crate) fn update_format(&self) {
        let node = self.imp().wpnode.get().expect("node");

        node.enum_params(Some("Format"), None, gtk::gio::Cancellable::NONE, clone!(@weak self as widget, @weak node => move |res| {
            if let Ok(Some(iter)) = res {
                let keys = wp::spa::SpaIdTable::from_name("Spa:Pod:Object:Param:Format").expect("id table");
                let channels_key = keys.find_value_from_short_name("channels").expect("channels key");
                let rate_key = keys.find_value_from_short_name("rate").expect("channels key");
                let format_key = keys.find_value_from_short_name("format").expect("format key");
                let position_key = keys.find_value_from_short_name("position").expect("position key");

                for a in iter {
                    let pod: wp::spa::SpaPod = a.get().unwrap();
                    if !pod.is_object() {
                        continue;
                    }

                    fn get_pod_maybe_choice(pod: wp::spa::SpaPod) -> wp::spa::SpaPod {
                        if pod.is_choice() {
                            pod.choice_child().unwrap()
                        } else {
                            pod
                        }
                    }

                    let choice = pod.find_spa_property(&format_key).expect("Format!");
                    let format = get_pod_maybe_choice(choice).id().expect("Format id");
                    if format == 0 {
                        wp::log::warning!("Format is 0, ignoring...");
                        return;
                    }

                    let choice = pod.find_spa_property(&channels_key).expect("Channels!");
                    let channels = get_pod_maybe_choice(choice).int().expect("Channels int");

                    let choice = pod.find_spa_property(&rate_key).expect("Rate!");
                    let rate = get_pod_maybe_choice(choice).int().expect("Rate int");

                    let choice = pod.find_spa_property(&position_key).expect("Position!");
                    let positionpod = get_pod_maybe_choice(choice);
                    let vec: Vec<u32> = positionpod.array_iterator().map(|x: i32| x as u32).collect();
                    let mut a = [0u32;64];
                    for (i,v) in (0..).zip(vec.iter()) {
                        a[i] = *v;
                    }

                    wp::log::info!("For id {}, Got rate {rate}, format {format}, channels {channels}", node.bound_id());

                    let t_format = wp::spa::SpaIdTable::from_name("Spa:Enum:AudioFormat").expect("audio format type");
                    let formatname = t_format.values().find(|x| x.number() == format).and_then(|x|x.short_name()).unwrap();

                    widget.set_formatstr(format!("{}ch {}Hz {}", channels, rate, formatname));

                    widget.set_format(AudioFormat { channels, format, rate, positions: a });
                }
            } else {
                wp::log::debug!("enum_params async call didn't return anything useful");
            }
            
        }));
    }

    pub(crate) fn update_mainvolume(&self) {
        let node = self.imp().wpnode.get().expect("node");

        let params = node
            .enum_params_sync("Props", None)
            .expect("getting params");

        let keys = wp::spa::SpaIdTable::from_name("Spa:Pod:Object:Param:Props").expect("id table");
        let volume_key = keys.find_value_from_short_name("volume").expect("volume key");

        for a in params {
            let pod: wp::spa::SpaPod = a.get().unwrap();
            if pod.is_object() {
                if let Some(val) = pod.find_spa_property(&volume_key) {
                    if let Some(volume) = val.float() {
                        self.set_mainvolume(volume);
                    }
                }
            }
        }
    }

    pub(crate) fn update_channel_volumes(&self) {
        let node = self.imp().wpnode.get().expect("node");
        let device_id = node.device_id().map_or(None, |x|x);

        let params = node
            .enum_params_sync("Props", None)
            .expect("getting params");

        let keys =
        wp::spa::SpaIdTable::from_name("Spa:Pod:Object:Param:Props").expect("id table");
        let channelvolumes_key = keys.find_value_from_short_name("channelVolumes").expect("channelVolumes key");
        let volume_key = keys.find_value_from_short_name("volume").expect("volume key");
        let mute_key = keys.find_value_from_short_name("mute").expect("mute key");

        for a in params {
            let pod: wp::spa::SpaPod = a.get().unwrap();
            if pod.is_object() {

                if let Some(val) = pod.find_spa_property(&channelvolumes_key) {
                    let mut volumes: Vec<f32> = Vec::new();
                    for a in val.array_iterator() {
                        volumes.push(a);
                    }
                    if volumes.len() == 0 {
                        wp::log::warning!("Got 0 channel volumes, ignoring...");
                        return;
                    }
                    self.set_channel_volumes_vec(&volumes);
                    if device_id.is_some() {
                        let maxvol: f32 = *volumes.iter().max_by(|a, b| a.total_cmp(b)).expect("Max");
                        self.set_volume(maxvol);
                    }
                }

                if device_id.is_none() {
                    if let Some(val) = pod.find_spa_property(&volume_key) {
                        if let Some(volume) = val.float() {
                            self.set_volume(volume);
                        }
                    }
                }

                if let Some(val) = pod.find_spa_property(&mute_key) {
                    if let Some(mute) = val.boolean() {
                        self.set_mute(mute);
                    }
                }
            }
        }
    }

    fn send_mainvolume(&self) {
        let podbuilder = SpaPodBuilder::new_object("Spa:Pod:Object:Param:Props", "Props");
        let node = self.imp().wpnode.get().expect("WpNode set");

        podbuilder.add_property("volume");
        podbuilder.add_float(self.mainvolume());

        if let Some(pod) = podbuilder.end() {
            node.set_param("Props", 0, pod);
        }

    }

    fn send_volume(&self, what: PropertyChanged) {
        let podbuilder = SpaPodBuilder::new_object("Spa:Pod:Object:Param:Props", "Props");
        let node = self.imp().wpnode.get().expect("WpNode set");

        let device_id = node.device_id().map_or(None, |x|x);

        match what {
            PropertyChanged::Volume => {
                // Device nodes don't really support the volume property.
                if device_id.is_none() {
                    podbuilder.add_property("volume");
                    podbuilder.add_float(self.volume());
                } else {

                    // Scale volumes according to current channel volumes
                    let channelspod = SpaPodBuilder::new_array();
                    let max = self.volume();
                    let t = *self.channel_volumes_vec().iter().max_by(|a, b| a.total_cmp(b)).expect("Max");
                    for v in self.channel_volumes_vec().iter() {
                        channelspod.add_float(*v * max / t);
                    }
                    if let Some(newpod) = channelspod.end() {
                        podbuilder.add_property("channelVolumes");
                        podbuilder.add_pod(&newpod);
                    }

                }
            },
            PropertyChanged::Mute => {
                podbuilder.add_property("mute");
                podbuilder.add_boolean(self.mute());
            },
            PropertyChanged::ChannelVolumes => {
                let channelspod = SpaPodBuilder::new_array();
                for v in self.channel_volumes_vec().iter() {
                    channelspod.add_float(*v);
                }
                if let Some(newpod) = channelspod.end() {
                    podbuilder.add_property("channelVolumes");
                    podbuilder.add_pod(&newpod);
                }
            },
        }

        if let Some(pod) = podbuilder.end() {

            // Check if this is a device node
            if let Some(id) = device_id {
                if let Ok(dev) = node.pw_property::<i32>("card.profile.device") {

                    if let Some(device) = Self::lookup_device_from_id(id) {
                        if let Some(idx) = Self::find_route_index(&device, dev) {
                            let builder = SpaPodBuilder::new_object("Spa:Pod:Object:Param:Route", "Route");
                            builder.add_property("index");
                            builder.add_int(idx);
                            builder.add_property("device");
                            builder.add_int(dev);
                            builder.add_property("props");
                            builder.add_pod(&pod);

                            if let Some(newpod) = builder.end() {
                                device.set_param("Route", 0, newpod);
                            }
                        } else {
                            wp::log::warning!("Cannot find route index");
                        }
                    } else {
                        wp::log::warning!("Cannot lookup device from id");
                    }
                } else {
                    wp::log::warning!("Cannot get card.profile.device");
                }
            } else {
                node.set_param("Props", 0, pod);
            }

        }
    }


    fn lookup_device_from_id(id: u32) -> Option<wp::pw::Device> {
        let app = PwvucontrolApplication::default();
        if let Some(r) = app.imp().devicemodel.into_iter().find(|x|{
            if let Ok(x) = x {
                let d: &wp::pw::Device = x.downcast_ref().expect("device");
                if d.bound_id() == id {
                    return true;
                }
            }
            false
        }) {
            if let Ok(d) = r {
                let device = d.dynamic_cast::<wp::pw::Device>();
                return device.ok();
            }
        }

/*         let om = app.imp().wp_object_manager.get().expect("Object manager set on application object");
        let interest = wp::registry::ObjectInterest::new_type(
            wp::pw::Device::static_type(),
        );
        interest.add_constraint(
            wp::registry::ConstraintType::GProperty,
            "bound-id",
            wp::registry::ConstraintVerb::Equals,
            Some(&id.to_variant()),
        );

        if let Some(obj) = om.lookup_full(interest) {
            return obj.dynamic_cast::<wp::pw::Device>().ok();
            
        } */

        None
    }

    fn find_route_index(obj: &wp::pw::Device, dev: i32) -> Option<i32> {
        if let Some(iter) = obj.enum_params_sync("Route", None) {
            let keys =
            wp::spa::SpaIdTable::from_name("Spa:Pod:Object:Param:Route").expect("id table");
            let index_key = keys.find_value_from_short_name("index").expect("index key");
            let device_key = keys.find_value_from_short_name("device").expect("device key");

            for a in iter {
                let pod: wp::spa::SpaPod = a.get().unwrap();

                let r_dev: Option<i32> = pod.spa_property(&device_key);
                let r_index: Option<i32> = pod.spa_property(&index_key);

                if let (Some(r_dev), Some(r_idx)) = (r_dev, r_index) {
                    if dev == r_dev {
                        return Some(r_idx);
                    }
                } else {
                    continue;
                }
            }
        }
        None
    }

    pub(crate) fn channel_volumes_vec(&self) -> Vec<f32> {
        self.imp().channel_volumes.borrow().clone()
    }

    pub(crate) fn set_channel_volumes_vec(&self, values: &Vec<f32>) {
        *(self.imp().channel_volumes.borrow_mut()) = values.clone();
        self.notify_channel_volumes();
    }

    pub(crate) fn set_channel_volume(&self, index: u32, volume: f32) {
        if let Some(value) = self
            .imp()
            .channel_volumes
            .borrow_mut()
            .get_mut(index as usize)
        {
            *value = volume;
        }
        self.notify_channel_volumes();
    }

    pub(crate) fn set_format(&self, format: AudioFormat) {
        self.imp().format.set(Some(format));

        self.emit_by_name::<()>("format", &[]);
    }

    pub(crate) fn format(&self) -> Option<AudioFormat> {
        self.imp().format.get()
    }

    pub(crate) fn set_default_target(&self, target_node: &PwNodeObject) {
        let app = PwvucontrolApplication::default();
        if let Some(metadata) = app.imp().metadata.borrow().as_ref() {
            metadata.set(self.boundid(), Some("target.node"), Some("Spa:Id"), Some(&target_node.boundid().to_string()));
            metadata.set(self.boundid(), Some("target.object"), Some("Spa:Id"), Some(&target_node.serial().to_string()));
        } else {
            wp::log::warning!("Cannot get metadata object");
        };
    }

    pub(crate) fn default_target(&self) -> Option<PwNodeObject> {
        let app = PwvucontrolApplication::default();
        let win = PwvucontrolWindow::default();
        let om = app.imp().wp_object_manager.get().unwrap();
        if let Some(metadata) = app.imp().metadata.borrow().as_ref() {
            if let Some(target_serial) = metadata.find_notype(self.boundid(), "target.object") {
                if target_serial != "-1" {
                    if let Some(sinknode) = om.lookup([
                        Constraint::compare(ConstraintType::PwProperty, "object.serial", target_serial.as_str(), true),
                    ].iter().collect::<Interest<wp::pw::Node>>()) {
                        return win.imp().nodemodel.get_node(sinknode.bound_id()).ok();
                    };
                }
            }
        } else {
            wp::log::warning!("Cannot get metadata object");
        };
        None
    }


    pub(crate) fn serial(&self) -> u32 {
        let node = self.imp().wpnode.get().expect("node");
        let serial: i32 = node.pw_property("object.serial").expect("object.serial");

        serial as u32
    }
}

trait MetadataExtFix: 'static {
    fn find_notype(&self, subject: u32, key: &str) -> Option<glib::GString>;
}

impl <O: glib::IsA<wp::pw::Metadata>> MetadataExtFix for O {
    fn find_notype(&self, subject: u32, key: &str) -> Option<glib::GString> {
        use glib::translate::ToGlibPtr;
        unsafe {
            let mut type_ = std::ptr::null();
            glib::translate::from_glib_none(wp::ffi::wp_metadata_find(self.as_ref().to_glib_none().0, subject, ToGlibPtr::to_glib_none(&key).0, &mut type_))
        }
    }
}