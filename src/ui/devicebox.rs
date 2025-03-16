// SPDX-License-Identifier: GPL-3.0-or-later

use crate::{backend::PwDeviceObject, ui::PwProfileDropDown};
use gtk::{prelude::*, subclass::prelude::*};
use std::cell::RefCell;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate, glib::Properties)]
    #[template(resource = "/com/saivert/pwvucontrol/gtk/devicebox.ui")]
    #[properties(wrapper_type = super::PwDeviceBox)]
    pub struct PwDeviceBox {
        #[template_child]
        pub icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub label: TemplateChild<gtk::Label>,
        #[template_child]
        pub profile_dropdown: TemplateChild<PwProfileDropDown>,

        #[property(get, set, construct_only)]
        pub deviceobject: RefCell<Option<PwDeviceObject>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PwDeviceBox {
        const NAME: &'static str = "PwDeviceBox";
        type Type = super::PwDeviceBox;
        type ParentType = gtk::ListBoxRow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for PwDeviceBox {
        fn constructed(&self) {
            self.parent_constructed();

            let obj = self.obj();

            let deviceobject = obj.deviceobject().expect("Device object");

            deviceobject.bind_property("name", &self.label.get(), "label").sync_create().build();

            deviceobject
                .bind_property("icon-name", &self.icon.get(), "icon-name")
                .sync_create()
                .build();

            self.profile_dropdown.set_deviceobject(obj.deviceobject());
        }
    }
    impl WidgetImpl for PwDeviceBox {}
    impl ListBoxRowImpl for PwDeviceBox {}

    impl PwDeviceBox {}
}

glib::wrapper! {
    pub struct PwDeviceBox(ObjectSubclass<imp::PwDeviceBox>)
        @extends gtk::Widget, gtk::ListBoxRow,
        @implements gtk::Actionable;
}

impl PwDeviceBox {
    pub(crate) fn new(deviceobject: &PwDeviceObject) -> Self {
        glib::Object::builder().property("deviceobject", deviceobject).build()
    }
}
