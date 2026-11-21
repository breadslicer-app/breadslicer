//! DragonFruit `src/supports/SupportPrimitives/ContactCone/`.

pub mod contact_cone_utils;
pub mod types;

pub use contact_cone_utils::{
    get_cone_center_position, get_cone_quaternion, get_final_socket_position,
    get_penetrated_contact_position, get_socket_position,
};
pub use types::{ContactCone, DEFAULT_TIP_PROFILE, SupportTipProfile};
