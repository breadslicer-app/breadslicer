/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::path::Path;

use crate::filetypes::{
    InferProjectFileTypeError, ProjectFileType, infer_project_file_type_from_extension,
};

mod infer_extension_tests {
    use super::*;

    #[test]
    fn threemf_extension_maps_to_its_variant() {
        assert_eq!(
            infer_project_file_type_from_extension(Path::new("plate.3mf")).unwrap(),
            ProjectFileType::ThreeMf,
        );
    }

    #[test]
    fn extension_match_is_case_insensitive() {
        assert_eq!(
            infer_project_file_type_from_extension(Path::new("plate.3MF")).unwrap(),
            ProjectFileType::ThreeMf,
        );
    }

    #[test]
    fn multiple_dots_use_the_last_segment() {
        assert_eq!(
            infer_project_file_type_from_extension(Path::new("plate.v2.3mf")).unwrap(),
            ProjectFileType::ThreeMf,
        );
    }

    #[test]
    fn unknown_extension_errors() {
        let err = infer_project_file_type_from_extension(Path::new("plate.stl")).unwrap_err();
        assert_eq!(
            err,
            InferProjectFileTypeError::UnknownExtension {
                path: Path::new("plate.stl").to_path_buf(),
            },
        );
    }

    #[test]
    fn path_without_extension_errors() {
        assert!(infer_project_file_type_from_extension(Path::new("noext")).is_err());
        assert!(infer_project_file_type_from_extension(Path::new("/tmp/")).is_err());
    }

    #[test]
    fn error_display_names_the_path() {
        let err = infer_project_file_type_from_extension(Path::new("plate.stl")).unwrap_err();
        assert_eq!(
            err.to_string(),
            "cannot infer a project file format from the extension of plate.stl",
        );
    }
}
