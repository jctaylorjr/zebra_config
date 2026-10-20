use crate::model::{Media, MediaType, Sensor};

// print width in dots: whole inches at the printer's resolution, never more than its nominal
// width. a ZD410 accepts 448 dots but is a 2 inch printer, so its widest is 2 x 203 = 406
pub fn width(m: &Media, dpi: u32, max_dots: u32) -> Result<u32, String> {
    let max_in = max_dots / dpi.max(1);
    let width_in = m.width_in.unwrap_or(max_in);
    if width_in == 0 || width_in > max_in {
        return Err(format!(
            "{width_in} inch media is wider than this printer's {max_in} inches"
        ));
    }
    Ok(width_in * dpi)
}

// SGD lines for a media profile
pub fn commands(m: &Media, dpi: u32, max_dots: u32) -> Result<Vec<String>, String> {
    let width = width(m, dpi, max_dots)?.to_string();
    let (media_type, sense_mode) = match m.media_type {
        MediaType::Gap => ("gap/notch", "gap"),
        MediaType::Mark => ("mark", "bar"),
    };
    let sensor = match m.sensor {
        Sensor::Transmissive => "transmissive",
        Sensor::Reflective => "reflective",
    };
    Ok([
        ("ezpl.media_type", media_type),
        ("media.sense_mode", sense_mode),
        ("device.sensor_select", sensor),
        ("ezpl.print_width", width.as_str()),
    ]
    .iter()
    .map(|(k, v)| format!(r#"! U1 setvar "{k}" "{v}""#))
    .collect())
}

// one label of left-aligned text, scaled so it's the same size at any dpi and fits a 1 inch label
pub fn test_label(lines: &[String], dpi: u32) -> String {
    let dots = |at_203: u32| at_203 * dpi / 203;
    let mut zpl = String::from("^XA^CI28");
    let mut y = dots(20);
    for (i, line) in lines.iter().enumerate() {
        let size = dots(if i == 0 { 32 } else { 24 });
        let text: String = line.chars().filter(|c| !matches!(c, '^' | '~')).collect();
        zpl += &format!("^FO{},{y}^A0N,{size},{size}^FD{text}^FS", dots(20));
        y += size + dots(6);
    }
    zpl + "^XZ"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(width_in: Option<u32>) -> Media {
        Media {
            width_in,
            media_type: MediaType::Gap,
            sensor: Sensor::Transmissive,
        }
    }

    fn width(lines: &[String]) -> &str {
        lines.last().unwrap().split('"').nth(3).unwrap()
    }

    #[test]
    fn widths_are_whole_inches_capped_at_the_printers_size() {
        // ZD410: 2 inch, 203 dpi, accepts up to 448 dots
        assert_eq!(width(&commands(&labels(Some(2)), 203, 448).unwrap()), "406");
        assert_eq!(width(&commands(&labels(None), 203, 448).unwrap()), "406");
        assert!(commands(&labels(Some(4)), 203, 448).is_err());
        // ZD621: 4 inch at 203 and 300 dpi
        assert_eq!(width(&commands(&labels(Some(4)), 203, 832).unwrap()), "812");
        assert_eq!(width(&commands(&labels(None), 203, 832).unwrap()), "812");
        assert_eq!(
            width(&commands(&labels(Some(2)), 300, 1280).unwrap()),
            "600"
        );
        assert_eq!(width(&commands(&labels(None), 300, 1280).unwrap()), "1200");
    }

    #[test]
    fn test_label_scales_with_dpi_and_drops_zpl_characters() {
        let lines = ["2x1 labels".to_string(), "a^b~c".to_string()];
        assert_eq!(
            test_label(&lines, 203),
            "^XA^CI28^FO20,20^A0N,32,32^FD2x1 labels^FS^FO20,58^A0N,24,24^FDabc^FS^XZ"
        );
        assert!(test_label(&lines, 300).contains("^FO29,29^A0N,47,47"));
    }

    #[test]
    fn wristbands_use_mark_sensing() {
        let m = Media {
            width_in: None,
            media_type: MediaType::Mark,
            sensor: Sensor::Reflective,
        };
        let lines = commands(&m, 203, 448).unwrap();
        assert_eq!(lines[0], r#"! U1 setvar "ezpl.media_type" "mark""#);
        assert_eq!(lines[1], r#"! U1 setvar "media.sense_mode" "bar""#);
        assert_eq!(
            lines[2],
            r#"! U1 setvar "device.sensor_select" "reflective""#
        );
    }
}
