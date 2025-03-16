// SPDX-License-Identifier: GPL-3.0-or-later

use super::{PwChannelObject, PwDeviceObject, PwRouteObject, PwvucontrolManager};
use glib::{clone, subclass::Signal, ParamSpec, Properties, Value};
use gtk::{gio, prelude::*, subclass::prelude::*};
use std::cell::OnceCell;
use std::cell::{Cell, RefCell};
use std::sync::OnceLock;
use wireplumber as wp;
use wp::registry::ObjectManager;
use wp::{
    pw::{FromPipewirePropertyString, GlobalProxyExt, MetadataExt, PipewireObjectExt, PipewireObjectExt2, ProxyExt},
    registry::{Constraint, ConstraintType, Interest},
    spa::SpaPodBuilder,
};

use crate::macros::*;

mod mixerapi;

#[derive(Debug, Copy, Clone, Eq, PartialEq, Default, glib::Enum)]
#[enum_type(name = "NodeType")]
pub enum NodeType {
    #[default]
    Undefined,
    StreamInput,
    StreamOutput,
    Sink,
    Source,
}

#[derive(Copy, Clone, Debug)]
pub struct AudioFormat {
    pub positions: [u32; 64],
}

pub(crate) enum PropertyChanged {
    Volume,
    Mute,
    ChannelVolumes,
}

pub mod imp {
    use super::*;

    #[derive(Properties)]
    #[properties(wrapper_type = super::PwNodeObject)]
    pub struct PwNodeObject {
        #[property(get, set)]
        name: RefCell<String>,
        #[property(get, set)]
        description: RefCell<String>,
        #[property(get, set)]
        formatstr: RefCell<String>,
        #[property(get, set)]
        boundid: Cell<u32>,
        #[property(get, set)]
        mainvolume: Cell<f32>,
        #[property(get, set)]
        volume: Cell<f32>,
        #[property(get, set)]
        monitorvolume: Cell<f32>,
        #[property(get, set)]
        mute: Cell<bool>,
        #[property(get, set)]
        iconname: RefCell<String>,
        #[property(get, set)]
        hidden: Cell<bool>,

        pub(super) channel_volumes: RefCell<Vec<f32>>,
        #[property(get, builder(NodeType::Undefined))]
        nodetype: Cell<NodeType>,

        #[property(get)]
        is_virtual: Cell<bool>,

        #[property(get)]
        pub(super) channelmodel: RefCell<gio::ListStore>,

        pub(super) format: Cell<Option<AudioFormat>>,

        #[property(get, set)]
        pub(super) channellock: Cell<bool>,

        #[property(get, set, construct_only)]
        pub(super) wpnode: OnceCell<wp::pw::Node>,

        pub(super) block: Cell<bool>,

        pub(super) om: RefCell<ObjectManager>,

        #[property(get)]
        pub(super) device: RefCell<Option<PwDeviceObject>>,
    }

    impl Default for PwNodeObject {
        fn default() -> Self {
            Self {
                name: Default::default(),
                description: Default::default(),
                formatstr: Default::default(),
                boundid: Default::default(),
                mainvolume: Default::default(),
                volume: Default::default(),
                monitorvolume: Default::default(),
                mute: Default::default(),
                iconname: Default::default(),
                channel_volumes: Default::default(),
                nodetype: Default::default(),
                channelmodel: RefCell::new(gio::ListStore::new::<PwChannelObject>()),
                format: Default::default(),
                channellock: Default::default(),
                wpnode: OnceCell::default(),
                block: Default::default(),
                om: Default::default(),
                hidden: Default::default(),
                device: Default::default(),
                is_virtual: Default::default(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PwNodeObject {
        const NAME: &'static str = "PwNodeObject";
        type Type = super::PwNodeObject;
    }

    impl ObjectImpl for PwNodeObject {
        fn properties() -> &'static [ParamSpec] {
            Self::derived_properties()
        }

        fn set_property(&self, id: usize, value: &Value, pspec: &ParamSpec) {
            self.derived_set_property(id, value, pspec);
            match pspec.name() {
                "volume" => {
                    if !self.block.get() {
                        self.obj().send_volume_using_mixerapi(PropertyChanged::Volume);
                    }
                }
                "mute" => {
                    if !self.block.get() {
                        self.obj().send_volume_using_mixerapi(PropertyChanged::Mute);
                    }
                }
                "mainvolume" => {
                    if !self.block.get() {
                        self.obj().send_mainvolume();
                    }
                }
                "monitorvolume" => {
                    if !self.block.get() {
                        self.obj().send_monitorvolume();
                    }
                }
                _ => {}
            }
        }

        fn property(&self, id: usize, pspec: &ParamSpec) -> Value {
            self.derived_property(id, pspec)
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| vec![Signal::builder("format").build()])
        }

        fn constructed(&self) {
            self.parent_constructed();

            let obj = self.obj();

            let node = self.wpnode.get().expect("Node set on PwNodeObject");

            self.nodetype.set(get_node_type_for_node(node));
            self.boundid.set(node.bound_id());

            if node.get_pw_property("media.class").as_deref() == Some("Audio/Source/Virtual") {
                self.is_virtual.set(true);
            }

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

            node.connect_params_changed(clone!(@weak obj => move |node,what| {
                pwvucontrol_debug!("params-changed! {what} id: {}", node.bound_id());
                obj.imp().block.set(true);
                match what {
                    "Props" => obj.update_props(),
                    "Format" => obj.update_format(),
                    _ => {},
                }
                obj.imp().block.set(false);
            }));

            obj.label_set_description();
            obj.update_props();
            obj.update_format();
            obj.label_set_name();

            obj.get_mixer_api();
            obj.update_volume_using_mixerapi();
            obj.update_icon_name();

            let om = self.om.borrow();

            om.add_interest(
                [Constraint::compare(ConstraintType::PwProperty, "link.output.node", node.bound_id(), true)]
                    .iter()
                    .collect::<Interest<wp::pw::Link>>(),
            );

            if let Ok(Some(device_id)) = node.device_id() {
                om.add_interest(
                    [Constraint::compare(ConstraintType::GProperty, "bound-id", device_id, true)]
                        .iter()
                        .collect::<Interest<wp::pw::Device>>(),
                );
            }

            om.connect_object_added(clone!(@weak self as nodeobject => move |_om, obj| {
                if let Some(link) = obj.downcast_ref::<wp::pw::Link>() {
                    let linked_node_id: u32 = link.pw_property("link.input.node").expect("link.input.node property");
                    let linked_node = PwvucontrolManager::default().get_node_by_id(linked_node_id);
                    pwvucontrol_info!("Node {} linked to node id {linked_node_id} ({:?})", nodeobject.obj().name(), linked_node.map(|x|x.name()));
                } else if let Some(device) = obj.downcast_ref::<wp::pw::Device>() {
                    let device_name: String = device.pw_property("device.description").unwrap();
                    let manager = PwvucontrolManager::default();
                    nodeobject.device.set(manager.get_device_by_id(device.bound_id()));
                    nodeobject.obj().notify_device();
                    pwvucontrol_info!("Node {} linked to device {device_name}", nodeobject.obj().name());
                }
            }));

            PwvucontrolManager::default().wp_core().install_object_manager(&om);

            // let manager = PwvucontrolManager::default();
            // manager.device_model().connect_items_changed(clone!(@weak self as nodeobject => move |liststore, _position, _removed, _added| {
            //     for device in liststore.iter::<PwDeviceObject>().map_while(Result::ok) {
            //         let node = nodeobject.wpnode.get().expect("node");
            //         if let Ok(Some(id)) = node.device_id() {
            //             if id == device.wpdevice().bound_id() {
            //                 nodeobject.device.set(Some(device.clone()));
            //             }
            //         }
            //     }
            // }));
        }
    }

    impl PwNodeObject {}
}

glib::wrapper! {
    pub struct PwNodeObject(ObjectSubclass<imp::PwNodeObject>);
}

pub(crate) fn get_node_type_for_node(node: &wp::pw::Node) -> NodeType {
    match node.get_pw_property("media.class").as_deref() {
        Some("Stream/Output/Audio") => NodeType::StreamOutput,
        Some("Stream/Input/Audio") => NodeType::StreamInput,
        Some("Audio/Source") => NodeType::Source,
        Some("Audio/Source/Virtual") => NodeType::Source,
        Some("Audio/Sink") => NodeType::Sink,
        _ => NodeType::Undefined,
    }
}

impl PwNodeObject {
    pub(crate) fn new(node: &wp::pw::Node) -> Self {
        glib::Object::builder().property("wpnode", node).build()
    }

    fn label_set_name(&self) {
        let wp_node = self.imp().wpnode.get().expect("Node widget should always have a wp_node");
        let props = wp_node.global_properties().expect("Node has no properties");

        let name_gstr = match self.nodetype() {
            NodeType::Sink | NodeType::Source => props
                .get("node.description")
                .or_else(|| props.get("node.nick"))
                .or_else(|| props.get("node.name")),
            _ => props
                .get("node.nick")
                .or_else(|| props.get("node.description"))
                .or_else(|| props.get("node.name")),
        };

        let name = name_gstr.as_ref().map(|name| name.as_str()).unwrap_or_default();

        self.set_name(name);
    }

    fn label_set_description(&self) {
        let wp_node = self.imp().wpnode.get().expect("Node widget should always have a wp_node");
        let props = wp_node.properties().expect("Node has no properties");
        let name_gstr = props.get("media.name");

        let name = name_gstr.as_ref().map(|name| name.as_str()).unwrap_or_default();

        self.set_description(name);
    }

    fn update_icon_name(&self) {
        match self.nodetype() {
            NodeType::StreamInput | NodeType::StreamOutput => {
                self.set_iconname("library-music-symbolic");
                let icon_props = ["media.icon-name", "window.icon-name", "application.icon-name"];
                for prop in icon_props {
                    if let Ok(appid) = self.wpnode().pw_property::<String>(prop) {
                        self.set_iconname(appid);
                    }
                }
            }
            NodeType::Source | NodeType::Sink => {
                self.set_iconname("soundcard-symbolic");
            }
            _ => {
                self.set_iconname("library-music-symbolic");
            }
        }
    }

    fn update_format(&self) {
        let node = self.imp().wpnode.get().expect("node");

        node.enum_params(Some("Format"), None, gtk::gio::Cancellable::NONE, clone!(@weak self as widget, @weak node => move |res| {
            if let Ok(Some(iter)) = res {

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

                    let choice = pod.find_spa_property(&wp::spa::ffi::SPA_FORMAT_AUDIO_format).expect("Format!");
                    let format = get_pod_maybe_choice(choice).id().expect("Format id");
                    if format == 0 {
                        pwvucontrol_warning!("Format is 0, ignoring...");
                        return;
                    }

                    let channels = match pod.find_spa_property(&wp::spa::ffi::SPA_FORMAT_AUDIO_channels) {
                        Some(pod) => get_pod_maybe_choice(pod).int().expect("Channels int"),
                        None => 0
                    };

                    let rate = match pod.find_spa_property(&wp::spa::ffi::SPA_FORMAT_AUDIO_rate) {
                        Some(pod) => get_pod_maybe_choice(pod).int().expect("Rate int"),
                        None => 0
                    };

                    let choice = pod.find_spa_property(&wp::spa::ffi::SPA_FORMAT_AUDIO_position).expect("Position!");
                    let positionpod = get_pod_maybe_choice(choice);
                    let vec: Vec<u32> = positionpod.array_iterator().map(|x: i32| x as u32).collect();
                    let mut a = [0u32;64];
                    for (i,v) in (0..).zip(vec.iter()) {
                        a[i] = *v;
                    }

                    pwvucontrol_info!("For {} bound id {}, Got rate {rate}, format {format}, channels {channels}", node.name().unwrap_or_default(), node.bound_id());

                    let t_format = wp::spa::SpaIdTable::from_name("Spa:Enum:AudioFormat").expect("audio format type");
                    let formatname = t_format.values().into_iter().find(|x| x.number() == format).and_then(|x|x.short_name()).unwrap();

                    widget.set_formatstr(format!("{}ch {}Hz {}", channels, rate, formatname));

                    widget.set_format(AudioFormat { positions: a });

                    // Must be done here since EnumFormat is async
                    widget.update_channelmodel();
                }
            } else {
                pwvucontrol_debug!("enum_params async call didn't return anything useful");
            }
        }));
    }

    pub(crate) fn update_props(&self) {
        let node = self.imp().wpnode.get().expect("node");

        let params = node.enum_params_sync("Props", None).expect("getting params");

        for a in params {
            let pod: wp::spa::SpaPod = a.get().unwrap();
            if pod.is_object() {
                if let Some(val) = pod.find_spa_property(&wp::spa::ffi::SPA_PROP_volume) {
                    if let Some(volume) = val.float() {
                        self.set_mainvolume(volume);
                    }
                }

                if let Some(val) = pod.find_spa_property(&wp::spa::ffi::SPA_PROP_monitorVolumes) {
                    if val.is_array() {
                        let volume = val.array_iterator::<f32>().max_by(f32::total_cmp);
                        self.set_monitorvolume(volume.unwrap_or_default());
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

    fn send_monitorvolume(&self) {
        let podbuilder = SpaPodBuilder::new_object("Spa:Pod:Object:Param:Props", "Props");
        let node = self.imp().wpnode.get().expect("WpNode set");

        let array = SpaPodBuilder::new_array();
        let volume = self.monitorvolume();
        for _ in 0..self.channel_volumes_vec().len() {
            array.add_float(volume);
        }
        if let Some(arraypod) = array.end() {
            podbuilder.add_property("monitorVolumes");
            podbuilder.add_pod(&arraypod);
        }

        if let Some(pod) = podbuilder.end() {
            node.set_param("Props", 0, pod);
        }
    }

    pub(crate) fn channel_volumes_vec(&self) -> Vec<f32> {
        self.imp().channel_volumes.borrow().clone()
    }

    pub(crate) fn set_channel_volumes_vec(&self, values: &[f32]) {
        self.set_channel_volumes_vec_no_send(values);
        if !self.imp().block.get() {
            self.send_volume_using_mixerapi(PropertyChanged::ChannelVolumes);
        }
    }

    pub(crate) fn set_channel_volumes_vec_no_send(&self, values: &[f32]) {
        *(self.imp().channel_volumes.borrow_mut()) = values.to_owned();

        self.update_channel_objects();
    }

    pub(crate) fn set_channel_volume(&self, index: u32, volume: f32) {
        if let Some(value) = self.imp().channel_volumes.borrow_mut().get_mut(index as usize) {
            *value = volume;
        }

        self.update_channel_objects();
        if !self.imp().block.get() {
            self.send_volume_using_mixerapi(PropertyChanged::ChannelVolumes);
        }
    }

    fn set_format(&self, format: AudioFormat) {
        self.imp().format.set(Some(format));

        self.emit_by_name::<()>("format", &[]);
    }

    pub(crate) fn format(&self) -> Option<AudioFormat> {
        self.imp().format.get()
    }

    pub(crate) fn set_default_target(&self, target_node: &PwNodeObject) {
        let manager = PwvucontrolManager::default();

        if let Some(metadata) = manager.metadata() {
            metadata.set(
                self.boundid(),
                Some("target.node"),
                Some("Spa:Id"),
                Some(&target_node.boundid().to_string()),
            );
            metadata.set(
                self.boundid(),
                Some("target.object"),
                Some("Spa:Id"),
                Some(&target_node.serial().to_string()),
            );
        } else {
            pwvucontrol_warning!("Cannot get metadata object");
        };
    }

    pub(crate) fn set_route(&self, routeobj: &PwRouteObject) {
        let index = routeobj.index();
        if let Ok(Some(card_profile_device)) = self.wpnode().device_index() {
            if let Some(device) = self.device() {
                device.set_route(index, card_profile_device as i32);

                let profiles = routeobj.get_profiles();
                if !profiles.is_empty() {
                    if !profiles.contains(&device.profile_index()) {
                        device.set_profile(profiles[0] as i32);
                    }
                }
            }
        }
    }

    pub(crate) fn default_target(&self) -> Option<PwNodeObject> {
        let manager = PwvucontrolManager::default();

        let om = manager.imp().wp_object_manager.get().unwrap();
        if let Some(metadata) = manager.metadata() {
            if let Some(target_serial) = metadata.find_notype(self.boundid(), "target.object") {
                if target_serial != "-1" {
                    if let Some(sinknode) = om.lookup(
                        [Constraint::compare(
                            ConstraintType::PwProperty,
                            "object.serial",
                            target_serial.as_str(),
                            true,
                        )]
                        .iter()
                        .collect::<Interest<wp::pw::Node>>(),
                    ) {
                        return manager.get_node_by_id(sinknode.bound_id());
                    };
                }
            }

            if let Some(target_node) = metadata.find_notype(self.boundid(), "target.node") {
                if target_node != "-1" {
                    if let Some(sinknode) = om.lookup(
                        [Constraint::compare(ConstraintType::PwProperty, "object.id", target_node.as_str(), true)]
                            .iter()
                            .collect::<Interest<wp::pw::Node>>(),
                    ) {
                        return manager.get_node_by_id(sinknode.bound_id());
                    };
                }
            }
        } else {
            pwvucontrol_warning!("Cannot get metadata object");
        };
        None
    }

    pub(crate) fn unset_default_target(&self) {
        let manager = PwvucontrolManager::default();

        if let Some(metadata) = manager.metadata() {
            metadata.set(self.boundid(), Some("target.node"), Some("Spa:Id"), Some("-1"));
            metadata.set(self.boundid(), Some("target.object"), Some("Spa:Id"), Some("-1"));
        } else {
            pwvucontrol_warning!("Cannot get metadata object");
        };
    }

    fn update_channelmodel(&self) {
        let channelmodel = self.imp().channelmodel.borrow();
        channelmodel.remove_all();
        for (index, item) in self.channel_volumes_vec().iter().enumerate() {
            let new_item = PwChannelObject::new(index as u32, *item, self);
            channelmodel.append(&new_item);
        }
    }

    fn update_channel_objects(&self) {
        let channelmodel = self.imp().channelmodel.borrow();
        for (index, vol) in self.channel_volumes_vec().iter().enumerate() {
            if let Some(channel_object) = channelmodel.item(index as u32).and_downcast_ref::<PwChannelObject>() {
                channel_object.set_volume_no_send(*vol);
            }
        }
    }

    pub(crate) fn serial(&self) -> u32 {
        let node = self.imp().wpnode.get().expect("node");
        let serial: i32 = node.pw_property("object.serial").expect("object.serial");

        serial as u32
    }

    pub(crate) fn node_property<T: FromPipewirePropertyString>(&self, property: &str) -> Option<T> {
        let node = self.imp().wpnode.get().expect("node");
        node.pw_property(property).ok()
    }
}

trait MetadataExtFix: 'static {
    fn find_notype(&self, subject: u32, key: &str) -> Option<glib::GString>;
}

impl<O: IsA<wp::pw::Metadata>> MetadataExtFix for O {
    fn find_notype(&self, subject: u32, key: &str) -> Option<glib::GString> {
        use glib::translate::ToGlibPtr;
        unsafe {
            let mut type_ = std::ptr::null();
            glib::translate::from_glib_none(wp::ffi::wp_metadata_find(
                self.as_ref().to_glib_none().0,
                subject,
                ToGlibPtr::to_glib_none(&key).0,
                &mut type_,
            ))
        }
    }
}
