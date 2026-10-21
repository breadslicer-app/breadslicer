/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Tests for [`ImageAssetRef`]: which strings are accepted, how a
//! reference resolves against a library root, and that the serde
//! shape stays a plain string.

use std::path::Path;

use crate::image_asset_ref::{ImageAssetRef, NonLibraryRelativeImageRefError};

#[test]
fn accepts_a_single_segment_relative_path() {
    let reference = ImageAssetRef::new("images/model.png").unwrap();
    assert_eq!(reference.as_str(), "images/model.png");
}

#[test]
fn accepts_nested_relative_paths() {
    assert!(ImageAssetRef::new("images/vendor/model.png").is_ok());
}

#[test]
fn accepts_a_bare_file_name() {
    assert!(ImageAssetRef::new("model.png").is_ok());
}

#[test]
fn rejects_the_empty_string() {
    assert_eq!(
        ImageAssetRef::new("").unwrap_err(),
        NonLibraryRelativeImageRefError::Empty
    );
}

#[test]
fn rejects_unix_absolute_paths() {
    assert_eq!(
        ImageAssetRef::new("/etc/model.png").unwrap_err(),
        NonLibraryRelativeImageRefError::Absolute
    );
}

#[test]
fn rejects_backslash_rooted_and_drive_prefixed_paths() {
    for input in ["\\images\\model.png", "C:/images/model.png", "c:model.png"] {
        assert_eq!(
            ImageAssetRef::new(input).unwrap_err(),
            NonLibraryRelativeImageRefError::Absolute,
            "{input}"
        );
    }
}

#[test]
fn rejects_parent_and_current_dir_components() {
    for input in [
        "../model.png",
        "images/../model.png",
        "./model.png",
        "images/./model.png",
    ] {
        assert_eq!(
            ImageAssetRef::new(input).unwrap_err(),
            NonLibraryRelativeImageRefError::NonNormalComponent,
            "{input}"
        );
    }
}

#[test]
fn rejects_empty_components() {
    for input in ["images//model.png", "images/model.png/"] {
        assert_eq!(
            ImageAssetRef::new(input).unwrap_err(),
            NonLibraryRelativeImageRefError::NonNormalComponent,
            "{input}"
        );
    }
}

#[test]
fn resolves_each_component_under_the_library_root() {
    let reference = ImageAssetRef::new("images/vendor/model.png").unwrap();
    let resolved = reference.resolve_in(Path::new("/lib"));
    assert_eq!(
        resolved,
        Path::new("/lib")
            .join("images")
            .join("vendor")
            .join("model.png")
    );
}

#[test]
fn from_str_and_try_from_agree_with_new() {
    let via_new = ImageAssetRef::new("images/model.png").unwrap();
    let via_str: ImageAssetRef = "images/model.png".parse().unwrap();
    let via_try = ImageAssetRef::try_from("images/model.png".to_owned()).unwrap();
    assert_eq!(via_new, via_str);
    assert_eq!(via_new, via_try);
    assert!("../x.png".parse::<ImageAssetRef>().is_err());
    assert!(ImageAssetRef::try_from(String::new()).is_err());
}

#[test]
fn serde_round_trips_as_a_plain_string() {
    #[derive(serde::Deserialize, serde::Serialize, PartialEq, Debug)]
    struct Holder {
        image: ImageAssetRef,
    }
    let holder = Holder {
        image: ImageAssetRef::new("images/model.png").unwrap(),
    };
    let toml = toml_edit::ser::to_string(&holder).unwrap();
    assert_eq!(toml.trim(), "image = \"images/model.png\"");
    let back: Holder = toml_edit::de::from_str(&toml).unwrap();
    assert_eq!(back, holder);
}

#[test]
fn serde_rejects_an_escaping_reference() {
    #[derive(serde::Deserialize, Debug)]
    #[allow(dead_code)]
    struct Holder {
        image: ImageAssetRef,
    }
    let err = toml_edit::de::from_str::<Holder>("image = \"../model.png\"").unwrap_err();
    assert!(err.to_string().contains("plain names"), "{err}");
}

#[test]
fn display_is_the_stored_string() {
    let reference = ImageAssetRef::new("images/model.png").unwrap();
    assert_eq!(reference.to_string(), "images/model.png");
    let string: String = reference.into();
    assert_eq!(string, "images/model.png");
}
