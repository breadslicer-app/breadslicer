/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

use super::*;

/// Draws a layer's lines back as UVtools' reader does.
fn draw(block: &str, width: usize, height: usize, mirror: bool) -> Vec<u8> {
    let after = block.split("FB\r\n").nth(1).unwrap();
    let coords: Vec<(i64, i64, u8)> = after
        .lines()
        .take_while(|line| *line != "FC")
        .map(|line| {
            let parts: Vec<i64> = line.split(',').map(|p| p.parse().unwrap()).collect();
            (parts[0], parts[1], parts[2] as u8)
        })
        .collect();
    let mut image = vec![0_u8; width * height];
    if coords.is_empty() {
        return image;
    }
    let mut zeros = 1;
    let mut start = (coords[0].0, coords[0].1);
    let mut end = start;
    for i in 1..coords.len() {
        let c = coords[i];
        if c.2 == 0 {
            zeros += 1;
            continue;
        }
        let last = coords[i - 1];
        if zeros == 1 {
            if i > 1 {
                start = (c.0, end.1 + last.1);
            }
        } else {
            start = (c.0, last.1);
        }
        end = (c.0, start.1 + c.1);
        let x = if mirror {
            width as i64 - start.0 + 1
        } else {
            start.0
        };
        for y in start.1..=end.1 {
            image[y as usize * width + x as usize] = 255;
        }
        zeros = 0;
    }
    image
}

#[test]
fn a_layer_reads_back_as_drawn_even_or_odd() {
    let (width, height) = (12_usize, 9_usize);
    let mut pixels = vec![0_u8; width * height];
    for (x, y) in [
        (1, 1),
        (1, 2),
        (1, 5),
        (4, 0),
        (4, 8),
        (10, 3),
        (10, 4),
        (11, 4),
    ] {
        pixels[y * width + x] = 255;
    }
    let mut runs: Vec<RleRun> = Vec::new();
    for &value in &pixels {
        match runs.last_mut() {
            Some(run) if run.value == value => run.length += 1,
            _ => runs.push(RleRun { value, length: 1 }),
        }
    }
    for index in [0, 1] {
        let block = layer_block(index, &runs, width as u32, height as u32);
        assert!(block.starts_with(&format!("{}\r\n", index + 1)));
        assert!(block.ends_with("FC\r\n"));
        assert_eq!(
            draw(&block, width, height, index % 2 != 0),
            pixels,
            "layer {index}"
        );
    }
}

#[test]
fn a_dark_layer_is_its_number_and_the_markers() {
    let runs = [RleRun {
        value: 0,
        length: 20,
    }];
    assert_eq!(layer_block(2, &runs, 5, 4), "3\r\nFB\r\nFC\r\n");
}
