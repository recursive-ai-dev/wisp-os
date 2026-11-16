//! Exercise the production renderer against guarded RAM instead of MMIO.
pub use wisp_host_tests::line_editor;
#[allow(dead_code)]
#[path = "../../../src/font.rs"]
mod font;
#[allow(dead_code)]
#[path = "../../../src/gfx.rs"]
mod gfx;

use gfx::{Color, Console};
use uefi::proto::console::gop::PixelFormat;

const GUARD: u32 = 0xa5a5a5a5;

#[test]
fn rejects_short_framebuffer_before_clearing_any_pixels() {
    let mut pixels = [GUARD; 8 * 16];
    let result = unsafe {
        Console::new(pixels.as_mut_ptr().cast(), 4, 8, 16, 8, PixelFormat::Bgr)
    };
    assert!(result.is_err());
    assert_eq!(pixels, [GUARD; 8 * 16]);
}

#[test]
fn padded_framebuffer_preserves_guard_words_and_pixel_order() {
    for format in [PixelFormat::Rgb, PixelFormat::Bgr] {
        let mut pixels = vec![GUARD; 12 * 32 + 2];
        let mut console = unsafe {
            Console::new(pixels.as_mut_ptr().add(1).cast(), 12 * 32 * 4,
                         8, 32, 12, format).unwrap()
        };
        console.plot_pixel(0, 0, Color::rgb(1, 2, 3));
        assert_eq!(pixels[1], if format == PixelFormat::Rgb { 0x030201 } else { 0x010203 });
        console.draw_rect_filled(-2, -2, 4, 4, Color::rgb(255, 0, 0));
        console.draw_line(-10, -10, 7, 31, Color::rgb(0, 255, 0));
        for _ in 0..10 {
            console.write_str("long line scrolling\n");
        }
        console.fill_screen(Color::rgb(3, 2, 1));
        console.clear();
        assert_eq!(pixels[0], GUARD);
        assert_eq!(*pixels.last().unwrap(), GUARD);
    }
}

#[test]
fn malformed_geometry_is_rejected_without_writes() {
    let mut pixels = [GUARD; 512];
    for (width, height, stride, len) in [
        (8, 16, 7, 2048), (0, 16, 8, 2048), (8, 0, 8, 2048),
        (8, 16, usize::MAX, 2048), (8, usize::MAX, 8, 2048),
        (8, 16, 8, 511),
    ] {
        assert!(unsafe { Console::new(pixels.as_mut_ptr().cast(), len,
            width, height, stride, PixelFormat::Bgr) }.is_err());
        assert_eq!(pixels, [GUARD; 512]);
    }
    assert!(unsafe { Console::new(core::ptr::null_mut(), 512, 8, 16, 8,
        PixelFormat::Bgr) }.is_err());
    assert!(unsafe { Console::new(pixels.as_mut_ptr().cast::<u8>().add(1),
        512, 8, 16, 8, PixelFormat::Bgr) }.is_err());
    assert_eq!(pixels, [GUARD; 512]);
}

#[test]
fn extreme_coordinates_cannot_bypass_work_bound_or_wrap_rectangles() {
    let mut pixels = [0u32; 8 * 16];
    let mut console = unsafe { Console::new(pixels.as_mut_ptr().cast(),
        512, 8, 16, 8, PixelFormat::Bgr).unwrap() };
    let white = Color::rgb(255, 255, 255);
    for (a, b) in [(i64::MIN, i64::MAX), (i64::MAX, i64::MIN)] {
        console.draw_line(a, 0, b, 0, white);
        console.draw_line(0, a, 0, b, white);
    }
    assert_eq!(pixels, [0; 8 * 16]);
    console.draw_rect_filled(i64::MIN, i64::MIN, usize::MAX, usize::MAX, white);
    assert!(pixels.iter().all(|&p| p == 0xffffff));
}

#[test]
fn seeded_clipped_lines_and_rectangles_never_touch_canaries() {
    let mut pixels = [GUARD; 10 * 32 + 2];
    let mut console = unsafe { Console::new(pixels.as_mut_ptr().add(1).cast(),
        10 * 32 * 4, 8, 32, 10, PixelFormat::Bgr).unwrap() };
    let mut state = 0x57495350u64;
    for _ in 0..2000 {
        let mut next = || {
            state ^= state << 13; state ^= state >> 7; state ^= state << 17;
            (state % 200) as i64 - 100
        };
        let (x, y, x2, y2) = (next(), next(), next(), next());
        console.draw_line(x, y, x2, y2, Color::rgb(1, 2, 3));
        console.draw_rect_filled(x, y, x2.unsigned_abs() as usize,
            y2.unsigned_abs() as usize, Color::rgb(3, 2, 1));
        assert_eq!(pixels[0], GUARD);
        assert_eq!(*pixels.last().unwrap(), GUARD);
    }
}

#[test]
fn scrollback_retains_scrolled_rows_and_restores_live_screen() {
    // 2 cols x 2 rows console (16x32 pixels).
    let mut pixels = vec![0u32; 16 * 32];
    let mut console = unsafe {
        Console::new(
            pixels.as_mut_ptr().cast(),
            16 * 32 * 4,
            16,
            32,
            16,
            PixelFormat::Bgr,
        )
        .unwrap()
    };
    console.write_str("AA\nBB\nCC");
    let live_snapshot = pixels.clone();
    assert!(!console.is_scrolled_back());

    console.scrollback_page_up();
    assert!(console.is_scrolled_back());
    // Scrolled back view must differ from the live bottom view ("AA" vs "BB"/"CC").
    assert_ne!(pixels, live_snapshot);

    assert!(console.exit_scrollback());
    assert!(!console.is_scrolled_back());
    assert_eq!(pixels, live_snapshot);
}
