/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

//! A flat outline with holes, for extrusion.

use super::vector2::Vector2;

/// A ring of points in the plane.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Path {
    pub points: Vec<Vector2>,
}

impl Path {
    pub fn new(points: Option<&[Vector2]>) -> Self {
        Path {
            points: points.map(<[Vector2]>::to_vec).unwrap_or_default(),
        }
    }

    /// A ring closes on itself: its last point joins its first. Kept so a
    /// caller can say so; the points need no closing duplicate.
    pub fn close_path(&mut self) -> &mut Self {
        self
    }

    /// The ring's points, consecutive duplicates skipped.
    pub fn get_points(&self) -> Vec<Vector2> {
        let mut points: Vec<Vector2> = Vec::with_capacity(self.points.len());
        for &p in &self.points {
            if points.last().is_none_or(|last| !last.equals(p)) {
                points.push(p);
            }
        }
        points
    }
}

/// An outline and its holes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Shape {
    pub path: Path,
    pub holes: Vec<Path>,
}

impl Shape {
    pub fn new(points: Option<&[Vector2]>) -> Self {
        Shape {
            path: Path::new(points),
            holes: Vec::new(),
        }
    }

    /// The outline's points and each hole's.
    pub fn extract_points(&self) -> (Vec<Vector2>, Vec<Vec<Vector2>>) {
        (
            self.path.get_points(),
            self.holes.iter().map(Path::get_points).collect(),
        )
    }
}
