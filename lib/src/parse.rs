use base64::Engine;
use glam::{Vec2, Vec3};
use nom::{
    branch::alt,
    bytes::complete::{tag, tag_no_case, take_while, take_while1, take_while_m_n},
    character::{
        complete::{digit1, line_ending as eol},
        is_digit,
    },
    combinator::{complete, map, map_res, opt},
    error::ErrorKind,
    multi::{many0, separated_list1},
    number::complete::float,
    sequence::{terminated, tuple},
    IResult, InputTakeAtPosition,
};
use std::str;

use crate::{
    Base64DataCmd, CategoryCmd, Color, ColorFinish, ColourCmd, Command, CommentCmd, DataCmd, Error,
    FileCmd, GlitterMaterial, GrainSize, KeywordsCmd, LineCmd, MaterialFinish, OptLineCmd,
    ParseError, QuadCmd, SpeckleMaterial, SubFileRefCmd, TriangleCmd,
};

// LDraw File Format Specification
// https://www.ldraw.org/article/218.html

pub fn parse_raw(ldr_content: &[u8]) -> Result<Vec<Command>, Error> {
    // "An LDraw file consists of one command per line."
    // TODO: What to set for the error message here?
    many0(read_line)(ldr_content).map_or_else(
        |e| Err(Error::Parse(ParseError::new_from_nom("", &e))),
        |(_, cmds)| Ok(cmds),
    )
}

fn nom_error(i: &[u8], kind: ErrorKind) -> nom::Err<nom::error::Error<&[u8]>> {
    nom::Err::Error(nom::error::Error::new(i, kind))
}

// "Whitespace is defined as one or more spaces (#32), tabs (#9), or combination thereof."
fn is_space(chr: u8) -> bool {
    chr == b'\t' || chr == b' '
}

fn take_spaces(i: &[u8]) -> IResult<&[u8], &[u8]> {
    take_while(is_space)(i)
}

// "All lines in the file must use the standard DOS/Windows line termination of <CR><LF>
// (carriage return/line feed). The file is permitted (but not required) to end with a <CR><LF>.
// It is recommended that all LDraw-compliant programs also be capable of reading files with the
// standard Unix line termination of <LF> (line feed)."
fn end_of_line(i: &[u8]) -> IResult<&[u8], &[u8]> {
    if i.is_empty() {
        Ok((i, i))
    } else {
        eol(i)
    }
}

// Detect a *potential* end of line <CR><LF> or <LF> by testing for either of <CR>
// and <LF>. Note that this doesn't necessarily means a proper end of line if <CR>
// is not followed by <LF>, but we assume this doesn't happen.
#[inline]
fn is_cr_or_lf(chr: u8) -> bool {
    chr == b'\n' || chr == b'\r'
}

// Parse any character which is not <CR> or <LF>, potentially until the end of input.
fn take_not_cr_or_lf(i: &[u8]) -> IResult<&[u8], &[u8]> {
    i.split_at_position_complete(is_cr_or_lf)
}

// Parse a single comma ',' character.
fn single_comma(i: &[u8]) -> IResult<&[u8], &[u8]> {
    if !i.is_empty() && (i[0] == b',') {
        Ok((&i[1..], &i[..1]))
    } else {
        // To work with separated_list!(), must return an Err::Error
        // when the separator doesn't parse anymore (and therefore
        // the list ends).
        Err(nom_error(i, nom::error::ErrorKind::Tag))
    }
}

// Parse any character which is not a comma ',' or <CR> or <LF>, potentially until the end of input.
// Invalid on empty input.
fn take_not_comma_or_eol(i: &[u8]) -> IResult<&[u8], &[u8]> {
    take_while1(|item| item != b',' && !is_cr_or_lf(item))(i)
}

// Parse any character which is not a space, potentially until the end of input.
fn take_not_space(i: &[u8]) -> IResult<&[u8], &[u8]> {
    i.split_at_position_complete(is_space)
}

// Read the command ID and swallow the following space, if any.
fn read_cmd_id_str(i: &[u8]) -> IResult<&[u8], &[u8]> {
    //terminated(take_while1(is_digit), sp)(i) //< This does not work if there's no space (e.g. 4-4cylo.dat)
    let (i, o) = i.split_at_position1_complete(|item| !is_digit(item), ErrorKind::Digit)?;
    let (i, _) = space0(i)?;
    Ok((i, o))
}

fn category(i: &[u8]) -> IResult<&[u8], Command> {
    let (i, _) = tag(b"!CATEGORY")(i)?;
    let (i, _) = sp(i)?;
    let (i, content) = map_res(take_not_cr_or_lf, str::from_utf8)(i)?;

    Ok((
        i,
        Command::Category(CategoryCmd {
            category: content.to_string(),
        }),
    ))
}

fn keywords_list(i: &[u8]) -> IResult<&[u8], Vec<&str>> {
    separated_list1(single_comma, map_res(take_not_comma_or_eol, str::from_utf8))(i)
}

fn keywords(i: &[u8]) -> IResult<&[u8], Command> {
    let (i, (_, _, keywords)) = tuple((tag(b"!KEYWORDS"), sp, keywords_list))(i)?;
    Ok((
        i,
        Command::Keywords(KeywordsCmd {
            keywords: keywords.iter().map(|kw| kw.trim().to_string()).collect(),
        }),
    ))
}

fn from_hex(i: &[u8]) -> Result<u8, nom::error::ErrorKind> {
    match std::str::from_utf8(i) {
        Ok(s) => match u8::from_str_radix(s, 16) {
            Ok(val) => Ok(val),
            Err(_) => Err(ErrorKind::AlphaNumeric),
        },
        Err(_) => Err(ErrorKind::AlphaNumeric),
    }
}

fn is_hex_digit(c: u8) -> bool {
    (c as char).is_ascii_hexdigit()
}

fn hex_primary(i: &[u8]) -> IResult<&[u8], u8> {
    map_res(take_while_m_n(2, 2, is_hex_digit), from_hex)(i)
}

fn hex_color(i: &[u8]) -> IResult<&[u8], Color> {
    let (i, _) = tag(b"#")(i)?;
    let (i, (red, green, blue)) = tuple((hex_primary, hex_primary, hex_primary))(i)?;
    Ok((i, Color { red, green, blue }))
}

fn digit1_as_u8(i: &[u8]) -> IResult<&[u8], u8> {
    map_res(map_res(digit1, str::from_utf8), str::parse::<u8>)(i)
}

// ALPHA part of !COLOUR
fn colour_alpha(i: &[u8]) -> IResult<&[u8], Option<u8>> {
    opt(complete(|i| {
        let (i, _) = sp(i)?;
        let (i, _) = tag(b"ALPHA")(i)?;
        let (i, _) = sp(i)?;
        digit1_as_u8(i)
    }))(i)
}

// LUMINANCE part of !COLOUR
fn colour_luminance(i: &[u8]) -> IResult<&[u8], Option<u8>> {
    opt(complete(|i| {
        let (i, _) = sp(i)?;
        let (i, _) = tag(b"LUMINANCE")(i)?;
        let (i, _) = sp(i)?;
        digit1_as_u8(i)
    }))(i)
}

fn material_grain_size(i: &[u8]) -> IResult<&[u8], GrainSize> {
    alt((grain_size, grain_min_max_size))(i)
}

fn grain_size(i: &[u8]) -> IResult<&[u8], GrainSize> {
    // TODO: Create tagged float helper?
    let (i, (_, _, size)) = tuple((tag(b"SIZE"), sp, float))(i)?;
    Ok((i, GrainSize::Size(size)))
}

fn grain_min_max_size(i: &[u8]) -> IResult<&[u8], GrainSize> {
    let (i, (_, _, min_size)) = tuple((tag(b"MINSIZE"), sp, float))(i)?;
    let (i, _) = sp(i)?;
    let (i, (_, _, max_size)) = tuple((tag(b"MAXSIZE"), sp, float))(i)?;
    Ok((i, GrainSize::MinMaxSize((min_size, max_size))))
}

// GLITTER VALUE v [ALPHA a] [LUMINANCE l] FRACTION f VFRACTION vf (SIZE s | MINSIZE min MAXSIZE max)
fn glitter_material(i: &[u8]) -> IResult<&[u8], ColorFinish> {
    let (i, _) = tag_no_case(b"GLITTER")(i)?;
    let (i, _) = sp(i)?;
    let (i, _) = tag_no_case(b"VALUE")(i)?;
    let (i, _) = sp(i)?;
    let (i, value) = hex_color(i)?;
    let (i, alpha) = colour_alpha(i)?;
    let (i, luminance) = colour_luminance(i)?;
    let (i, _) = sp(i)?;
    let (i, _) = tag_no_case(b"FRACTION")(i)?;
    let (i, _) = sp(i)?;
    let (i, surface_fraction) = float(i)?;
    let (i, _) = sp(i)?;
    let (i, _) = tag_no_case(b"VFRACTION")(i)?;
    let (i, _) = sp(i)?;
    let (i, volume_fraction) = float(i)?;
    let (i, _) = sp(i)?;
    let (i, size) = material_grain_size(i)?;

    Ok((
        i,
        ColorFinish::Material(MaterialFinish::Glitter(GlitterMaterial {
            value,
            alpha,
            luminance,
            surface_fraction,
            volume_fraction,
            size,
        })),
    ))
}

// SPECKLE VALUE v [ALPHA a] [LUMINANCE l] FRACTION f (SIZE s | MINSIZE min MAXSIZE max)
fn speckle_material(i: &[u8]) -> IResult<&[u8], ColorFinish> {
    let (i, _) = tag_no_case(b"SPECKLE")(i)?;
    let (i, _) = sp(i)?;
    let (i, _) = tag_no_case(b"VALUE")(i)?;
    let (i, _) = sp(i)?;
    let (i, value) = hex_color(i)?;
    let (i, alpha) = colour_alpha(i)?;
    let (i, luminance) = colour_luminance(i)?;
    let (i, _) = sp(i)?;
    let (i, _) = tag_no_case(b"FRACTION")(i)?;
    let (i, _) = sp(i)?;
    let (i, surface_fraction) = float(i)?;
    let (i, _) = sp(i)?;
    let (i, size) = material_grain_size(i)?;

    Ok((
        i,
        ColorFinish::Material(MaterialFinish::Speckle(SpeckleMaterial {
            value,
            alpha,
            luminance,
            surface_fraction,
            size,
        })),
    ))
}

// Other unrecognized MATERIAL definition
fn other_material(i: &[u8]) -> IResult<&[u8], ColorFinish> {
    let (i, content) = map_res(take_not_cr_or_lf, str::from_utf8)(i)?;
    let finish = content.trim().to_string();
    Ok((i, ColorFinish::Material(MaterialFinish::Other(finish))))
}

// MATERIAL finish part of !COLOUR
fn material_finish(i: &[u8]) -> IResult<&[u8], ColorFinish> {
    let (i, _) = tag_no_case(b"MATERIAL")(i)?;
    let (i, _) = sp(i)?;
    alt((glitter_material, speckle_material, other_material))(i)
}

// Finish part of !COLOUR
// TODO: Avoid having the leading space in each parser?
fn color_finish(i: &[u8]) -> IResult<&[u8], Option<ColorFinish>> {
    opt(complete(|i| {
        let (i, _) = sp(i)?;
        alt((
            map(tag_no_case(b"CHROME"), |_| ColorFinish::Chrome),
            map(tag_no_case(b"PEARLESCENT"), |_| ColorFinish::Pearlescent),
            map(tag_no_case(b"RUBBER"), |_| ColorFinish::Rubber),
            map(tag_no_case(b"MATTE_METALLIC"), |_| {
                ColorFinish::MatteMetallic
            }),
            map(tag_no_case(b"METAL"), |_| ColorFinish::Metal),
            material_finish,
        ))(i)
    }))(i)
}

// !COLOUR extension meta-command
fn meta_colour(i: &[u8]) -> IResult<&[u8], Command> {
    let (i, _) = tag(b"!COLOUR")(i)?;
    let (i, _) = sp(i)?;
    let (i, name) = map_res(take_not_space, str::from_utf8)(i)?;
    let (i, _) = sp(i)?;
    let (i, _) = tag(b"CODE")(i)?;
    let (i, _) = sp(i)?;
    let (i, code) = color_id(i)?;
    let (i, _) = sp(i)?;
    let (i, _) = tag(b"VALUE")(i)?;
    let (i, _) = sp(i)?;
    let (i, value) = hex_color(i)?;
    let (i, _) = sp(i)?;
    let (i, _) = tag(b"EDGE")(i)?;
    let (i, _) = sp(i)?;
    let (i, edge) = hex_color(i)?;
    let (i, alpha) = colour_alpha(i)?;
    let (i, luminance) = colour_luminance(i)?;
    let (i, finish) = color_finish(i)?;

    Ok((
        i,
        Command::Colour(ColourCmd {
            name: name.to_string(),
            code,
            value,
            edge,
            alpha,
            luminance,
            finish,
        }),
    ))
}

fn comment(i: &[u8]) -> IResult<&[u8], Command> {
    let (i, comment) = map_res(take_not_cr_or_lf, str::from_utf8)(i)?;
    Ok((i, Command::Comment(CommentCmd::new(comment))))
}

fn meta_file(i: &[u8]) -> IResult<&[u8], Command> {
    let (i, _) = tag(b"FILE")(i)?;
    let (i, _) = sp(i)?;
    let (i, file) = map_res(take_not_cr_or_lf, str::from_utf8)(i)?;

    Ok((
        i,
        Command::File(FileCmd {
            file: file.to_string(),
        }),
    ))
}

fn meta_data(i: &[u8]) -> IResult<&[u8], Command> {
    let (i, _) = tag(b"!DATA")(i)?;
    let (i, _) = sp(i)?;
    let (i, file) = map_res(take_not_cr_or_lf, str::from_utf8)(i)?;

    Ok((
        i,
        Command::Data(DataCmd {
            file: file.to_string(),
        }),
    ))
}

fn meta_base_64_data(i: &[u8]) -> IResult<&[u8], Command> {
    // TODO: Validate base64 characters?
    let (i, _) = tag(b"!:")(i)?;
    let (i, _) = sp(i)?;
    let (i, data) = map_res(take_not_cr_or_lf, |b| {
        base64::engine::general_purpose::STANDARD_NO_PAD.decode(b)
    })(i)?;

    Ok((i, Command::Base64Data(Base64DataCmd { data })))
}

fn meta_nofile(i: &[u8]) -> IResult<&[u8], Command> {
    let (i, _) = tag(b"NOFILE")(i)?;
    Ok((i, Command::NoFile))
}

fn meta_cmd(i: &[u8]) -> IResult<&[u8], Command> {
    alt((
        complete(category),
        complete(keywords),
        complete(meta_colour),
        complete(meta_file),
        complete(meta_nofile),
        complete(meta_data),
        complete(meta_base_64_data),
        comment,
    ))(i)
}

fn read_vec2(i: &[u8]) -> IResult<&[u8], Vec2> {
    let (i, (x, _, y)) = tuple((float, sp, float))(i)?;
    Ok((i, Vec2 { x, y }))
}

fn read_vec3(i: &[u8]) -> IResult<&[u8], Vec3> {
    let (i, (x, _, y, _, z)) = tuple((float, sp, float, sp, float))(i)?;
    Ok((i, Vec3 { x, y, z }))
}

fn color_id(i: &[u8]) -> IResult<&[u8], u32> {
    map_res(map_res(digit1, str::from_utf8), str::parse::<u32>)(i)
}

fn filename(i: &[u8]) -> IResult<&[u8], &str> {
    // Assume leading and trailing whitespace isn't part of the filename.
    map(map_res(take_not_cr_or_lf, str::from_utf8), |s| s.trim())(i)
}

fn file_ref_cmd(i: &[u8]) -> IResult<&[u8], Command> {
    let (i, color) = color_id(i)?;
    let (i, _) = sp(i)?;
    let (i, pos) = read_vec3(i)?;
    let (i, _) = sp(i)?;
    let (i, row0) = read_vec3(i)?;
    let (i, _) = sp(i)?;
    let (i, row1) = read_vec3(i)?;
    let (i, _) = sp(i)?;
    let (i, row2) = read_vec3(i)?;
    let (i, _) = sp(i)?;
    let (i, file) = filename(i)?;

    Ok((
        i,
        Command::SubFileRef(SubFileRefCmd {
            color,
            pos,
            row0,
            row1,
            row2,
            file: file.into(),
        }),
    ))
}

fn line_cmd(i: &[u8]) -> IResult<&[u8], Command> {
    let (i, color) = color_id(i)?;
    let (i, _) = sp(i)?;
    let (i, v1) = read_vec3(i)?;
    let (i, _) = sp(i)?;
    let (i, v2) = read_vec3(i)?;
    let (i, _) = space0(i)?;

    Ok((
        i,
        Command::Line(LineCmd {
            color,
            vertices: [v1, v2],
        }),
    ))
}

fn tri_cmd(i: &[u8]) -> IResult<&[u8], Command> {
    let (i, color) = color_id(i)?;
    let (i, _) = sp(i)?;
    let (i, v1) = read_vec3(i)?;
    let (i, _) = sp(i)?;
    let (i, v2) = read_vec3(i)?;
    let (i, _) = sp(i)?;
    let (i, v3) = read_vec3(i)?;
    let (i, _) = space0(i)?;

    let (i, uvs) = opt(complete(|i| {
        let (i, uv1) = read_vec2(i)?;
        let (i, _) = sp(i)?;
        let (i, uv2) = read_vec2(i)?;
        let (i, _) = sp(i)?;
        let (i, uv3) = read_vec2(i)?;
        let (i, _) = space0(i)?;
        Ok((i, [uv1, uv2, uv3]))
    }))(i)?;

    Ok((
        i,
        Command::Triangle(TriangleCmd {
            color,
            vertices: [v1, v2, v3],
            uvs,
        }),
    ))
}

fn quad_cmd(i: &[u8]) -> IResult<&[u8], Command> {
    let (i, color) = color_id(i)?;
    let (i, _) = sp(i)?;
    let (i, v1) = read_vec3(i)?;
    let (i, _) = sp(i)?;
    let (i, v2) = read_vec3(i)?;
    let (i, _) = sp(i)?;
    let (i, v3) = read_vec3(i)?;
    let (i, _) = sp(i)?;
    let (i, v4) = read_vec3(i)?;
    let (i, _) = space0(i)?;

    let (i, uvs) = opt(complete(|i| {
        let (i, uv1) = read_vec2(i)?;
        let (i, _) = sp(i)?;
        let (i, uv2) = read_vec2(i)?;
        let (i, _) = sp(i)?;
        let (i, uv3) = read_vec2(i)?;
        let (i, _) = space0(i)?;
        let (i, uv4) = read_vec2(i)?;
        let (i, _) = space0(i)?;
        Ok((i, [uv1, uv2, uv3, uv4]))
    }))(i)?;

    Ok((
        i,
        Command::Quad(QuadCmd {
            color,
            vertices: [v1, v2, v3, v4],
            uvs,
        }),
    ))
}

fn opt_line_cmd(i: &[u8]) -> IResult<&[u8], Command> {
    let (i, color) = color_id(i)?;
    let (i, _) = sp(i)?;
    let (i, v1) = read_vec3(i)?;
    let (i, _) = sp(i)?;
    let (i, v2) = read_vec3(i)?;
    let (i, _) = sp(i)?;
    let (i, v3) = read_vec3(i)?;
    let (i, _) = sp(i)?;
    let (i, v4) = read_vec3(i)?;
    let (i, _) = space0(i)?;

    Ok((
        i,
        Command::OptLine(OptLineCmd {
            color,
            vertices: [v1, v2],
            control_points: [v3, v4],
        }),
    ))
}

// Zero or more "spaces", as defined in LDraw standard.
// Valid even on empty input.
fn space0(i: &[u8]) -> IResult<&[u8], &[u8]> {
    i.split_at_position_complete(|item| !is_space(item))
}

// One or more "spaces", as defined in LDraw standard.
// Valid even on empty input.
fn sp(i: &[u8]) -> IResult<&[u8], &[u8]> {
    i.split_at_position1_complete(|item| !is_space(item), ErrorKind::Space)
}

// Zero or more "spaces", as defined in LDraw standard.
// Valid even on empty input.
fn space_or_eol0(i: &[u8]) -> IResult<&[u8], &[u8]> {
    i.split_at_position_complete(|item| !is_space(item) && !is_cr_or_lf(item))
}

// An empty line made of optional spaces, and ending with an end-of-line sequence
// (either <CR><LF> or <LF> alone) or the end of input.
// Valid even on empty input.
fn empty_line(i: &[u8]) -> IResult<&[u8], &[u8]> {
    terminated(space0, end_of_line)(i)
}

// "There is no line length restriction. Each command consists of optional leading
// whitespace followed by whitespace-delimited tokens. Some commands also have trailing
// arbitrary data which may itself include internal whitespace; such data is not tokenized,
// but treated as single unit according to the command."
//
// "Lines may also be empty or consist only of whitespace. Such lines have no effect."
//
// "The line type of a line is the first number on the line."
// "If the line type of the command is invalid, the line is ignored."
fn read_line(i: &[u8]) -> IResult<&[u8], Command> {
    let (i, _) = space_or_eol0(i)?;
    let (i, cmd_id) = read_cmd_id_str(i)?;
    let (i, cmd) = match cmd_id {
        b"0" => meta_cmd(i),
        b"1" => file_ref_cmd(i),
        b"2" => line_cmd(i),
        b"3" => tri_cmd(i),
        b"4" => quad_cmd(i),
        b"5" => opt_line_cmd(i),
        _ => Err(nom_error(i, ErrorKind::Switch)),
    }?;
    Ok((i, cmd))
}

#[cfg(test)]
mod tests {
    use nom::error::ErrorKind;

    use super::*;

    #[test]
    fn test_color_id() {
        assert_eq!(color_id(b""), Err(nom_error(&b""[..], ErrorKind::Digit)));
        assert_eq!(color_id(b"1"), Ok((&b""[..], 1)));
        assert_eq!(color_id(b"16 "), Ok((&b" "[..], 16)));
    }

    #[test]
    fn test_from_hex() {
        assert_eq!(from_hex(b"0"), Ok(0));
        assert_eq!(from_hex(b"1"), Ok(1));
        assert_eq!(from_hex(b"a"), Ok(10));
        assert_eq!(from_hex(b"F"), Ok(15));
        assert_eq!(from_hex(b"G"), Err(ErrorKind::AlphaNumeric));
        assert_eq!(from_hex(b"10"), Ok(16));
        assert_eq!(from_hex(b"FF"), Ok(255));
        assert_eq!(from_hex(b"1G"), Err(ErrorKind::AlphaNumeric));
        assert_eq!(from_hex(b"100"), Err(ErrorKind::AlphaNumeric));
        assert_eq!(from_hex(b"\xFF"), Err(ErrorKind::AlphaNumeric));
    }

    #[test]
    fn test_hex_color() {
        assert_eq!(hex_color(b""), Err(nom_error(&b""[..], ErrorKind::Tag)));
        assert_eq!(
            hex_color(b"#"),
            Err(nom_error(&b""[..], ErrorKind::TakeWhileMN))
        );
        assert_eq!(
            hex_color(b"#1"),
            Err(nom_error(&b"1"[..], ErrorKind::TakeWhileMN))
        );
        assert_eq!(
            hex_color(b"#12345Z"),
            Err(nom_error(&b"5Z"[..], ErrorKind::TakeWhileMN))
        );
        assert_eq!(
            hex_color(b"#123456"),
            Ok((&b""[..], Color::new(0x12, 0x34, 0x56)))
        );
        assert_eq!(
            hex_color(b"#ABCDEF"),
            Ok((&b""[..], Color::new(0xAB, 0xCD, 0xEF)))
        );
        assert_eq!(
            hex_color(b"#8E5cAf"),
            Ok((&b""[..], Color::new(0x8E, 0x5C, 0xAF)))
        );
        assert_eq!(
            hex_color(b"#123456e"),
            Ok((&b"e"[..], Color::new(0x12, 0x34, 0x56)))
        );
    }

    #[test]
    fn test_colour_alpha() {
        assert_eq!(colour_alpha(b""), Ok((&b""[..], None)));
        assert_eq!(colour_alpha(b" ALPHA 0"), Ok((&b""[..], Some(0))));
        assert_eq!(colour_alpha(b" ALPHA 1"), Ok((&b""[..], Some(1))));
        assert_eq!(colour_alpha(b" ALPHA 128"), Ok((&b""[..], Some(128))));
        assert_eq!(colour_alpha(b" ALPHA 255"), Ok((&b""[..], Some(255))));
        assert_eq!(colour_alpha(b" ALPHA 34 "), Ok((&b" "[..], Some(34))));
        // TODO - Should fail on partial match, but succeeds because of opt!()
        assert_eq!(colour_alpha(b" ALPHA"), Ok((&b" ALPHA"[..], None))); // Err(Err::Incomplete(Needed::Size(1)))
        assert_eq!(colour_alpha(b" ALPHA 256"), Ok((&b" ALPHA 256"[..], None)));
        // Err(Err::Incomplete(Needed::Size(1)))
    }

    #[test]
    fn test_colour_luminance() {
        assert_eq!(colour_luminance(b""), Ok((&b""[..], None)));
        assert_eq!(colour_luminance(b" LUMINANCE 0"), Ok((&b""[..], Some(0))));
        assert_eq!(colour_luminance(b" LUMINANCE 1"), Ok((&b""[..], Some(1))));
        assert_eq!(
            colour_luminance(b" LUMINANCE 128"),
            Ok((&b""[..], Some(128)))
        );
        assert_eq!(
            colour_luminance(b" LUMINANCE 255"),
            Ok((&b""[..], Some(255)))
        );
        assert_eq!(
            colour_luminance(b" LUMINANCE 34 "),
            Ok((&b" "[..], Some(34)))
        );
        // TODO - Should fail on partial match, but succeeds because of opt!()
        assert_eq!(
            colour_luminance(b" LUMINANCE"),
            Ok((&b" LUMINANCE"[..], None))
        ); // Err(Err::Incomplete(Needed::Size(1)))
        assert_eq!(
            colour_luminance(b" LUMINANCE 256"),
            Ok((&b" LUMINANCE 256"[..], None))
        ); // Err(Err::Incomplete(Needed::Size(1)))
    }

    #[test]
    fn test_material_grain_size() {
        assert_eq!(
            material_grain_size(b""),
            Err(nom_error(&b""[..], ErrorKind::Tag))
        );
        assert_eq!(
            material_grain_size(b"SIZE"),
            Err(nom_error(&b"SIZE"[..], ErrorKind::Tag))
        );
        assert_eq!(
            material_grain_size(b"SIZE 1"),
            Ok((&b""[..], GrainSize::Size(1.0)))
        );
        assert_eq!(
            material_grain_size(b"SIZE 0.02"),
            Ok((&b""[..], GrainSize::Size(0.02)))
        );
        assert_eq!(
            material_grain_size(b"MINSIZE"),
            Err(nom_error(&b""[..], ErrorKind::Space))
        );
        assert_eq!(
            material_grain_size(b"MINSIZE 0.02"),
            Err(nom_error(&b""[..], ErrorKind::Space))
        );
        assert_eq!(
            material_grain_size(b"MINSIZE 0.02 MAXSIZE 0.04"),
            Ok((&b""[..], GrainSize::MinMaxSize((0.02, 0.04))))
        );
    }

    #[test]
    fn test_glitter_material() {
        assert_eq!(
            glitter_material(b""),
            Err(nom_error(&b""[..], ErrorKind::Tag))
        );
        assert_eq!(
            glitter_material(b"GLITTER"),
            Err(nom_error(&b""[..], ErrorKind::Space))
        );
        assert_eq!(
            glitter_material(b"GLITTER VALUE #123456 FRACTION 1.0 VFRACTION 0.3 SIZE 1"),
            Ok((
                &b""[..],
                ColorFinish::Material(MaterialFinish::Glitter(GlitterMaterial {
                    value: Color::new(0x12, 0x34, 0x56),
                    alpha: None,
                    luminance: None,
                    surface_fraction: 1.0,
                    volume_fraction: 0.3,
                    size: GrainSize::Size(1.0)
                }))
            ))
        );
        assert_eq!(
            glitter_material(b"GLITTER VALUE #123456 ALPHA 128 FRACTION 1.0 VFRACTION 0.3 SIZE 1"),
            Ok((
                &b""[..],
                ColorFinish::Material(MaterialFinish::Glitter(GlitterMaterial {
                    value: Color::new(0x12, 0x34, 0x56),
                    alpha: Some(128),
                    luminance: None,
                    surface_fraction: 1.0,
                    volume_fraction: 0.3,
                    size: GrainSize::Size(1.0)
                }))
            ))
        );
        assert_eq!(
            glitter_material(
                b"GLITTER VALUE #123456 LUMINANCE 32 FRACTION 1.0 VFRACTION 0.3 SIZE 1"
            ),
            Ok((
                &b""[..],
                ColorFinish::Material(MaterialFinish::Glitter(GlitterMaterial {
                    value: Color::new(0x12, 0x34, 0x56),
                    alpha: None,
                    luminance: Some(32),
                    surface_fraction: 1.0,
                    volume_fraction: 0.3,
                    size: GrainSize::Size(1.0)
                }))
            ))
        );
        assert_eq!(
            glitter_material(
                b"GLITTER VALUE #123456 FRACTION 1.0 VFRACTION 0.3 MINSIZE 0.02 MAXSIZE 0.04"
            ),
            Ok((
                &b""[..],
                ColorFinish::Material(MaterialFinish::Glitter(GlitterMaterial {
                    value: Color::new(0x12, 0x34, 0x56),
                    alpha: None,
                    luminance: None,
                    surface_fraction: 1.0,
                    volume_fraction: 0.3,
                    size: GrainSize::MinMaxSize((0.02, 0.04))
                }))
            ))
        );
    }

    #[test]
    fn test_speckle_material() {
        assert_eq!(
            speckle_material(b""),
            Err(nom_error(&b""[..], ErrorKind::Tag))
        );
        assert_eq!(
            speckle_material(b"SPECKLE"),
            Err(nom_error(&b""[..], ErrorKind::Space))
        );
        assert_eq!(
            speckle_material(b"SPECKLE VALUE #123456 FRACTION 1.0 SIZE 1"),
            Ok((
                &b""[..],
                ColorFinish::Material(MaterialFinish::Speckle(SpeckleMaterial {
                    value: Color::new(0x12, 0x34, 0x56),
                    alpha: None,
                    luminance: None,
                    surface_fraction: 1.0,
                    size: GrainSize::Size(1.0)
                }))
            ))
        );
        assert_eq!(
            speckle_material(b"SPECKLE VALUE #123456 ALPHA 128 FRACTION 1.0 SIZE 1"),
            Ok((
                &b""[..],
                ColorFinish::Material(MaterialFinish::Speckle(SpeckleMaterial {
                    value: Color::new(0x12, 0x34, 0x56),
                    alpha: Some(128),
                    luminance: None,
                    surface_fraction: 1.0,
                    size: GrainSize::Size(1.0)
                }))
            ))
        );
        assert_eq!(
            speckle_material(b"SPECKLE VALUE #123456 LUMINANCE 32 FRACTION 1.0 SIZE 1"),
            Ok((
                &b""[..],
                ColorFinish::Material(MaterialFinish::Speckle(SpeckleMaterial {
                    value: Color::new(0x12, 0x34, 0x56),
                    alpha: None,
                    luminance: Some(32),
                    surface_fraction: 1.0,
                    size: GrainSize::Size(1.0)
                }))
            ))
        );
        assert_eq!(
            speckle_material(b"SPECKLE VALUE #123456 FRACTION 1.0 MINSIZE 0.02 MAXSIZE 0.04"),
            Ok((
                &b""[..],
                ColorFinish::Material(MaterialFinish::Speckle(SpeckleMaterial {
                    value: Color::new(0x12, 0x34, 0x56),
                    alpha: None,
                    luminance: None,
                    surface_fraction: 1.0,
                    size: GrainSize::MinMaxSize((0.02, 0.04))
                }))
            ))
        );
    }

    #[test]
    fn test_color_finish() {
        assert_eq!(color_finish(b""), Ok((&b""[..], None)));
        assert_eq!(color_finish(b"CHROME"), Ok((&b"CHROME"[..], None)));
        assert_eq!(
            color_finish(b" CHROME"),
            Ok((&b""[..], Some(ColorFinish::Chrome)))
        );
        assert_eq!(
            color_finish(b" PEARLESCENT"),
            Ok((&b""[..], Some(ColorFinish::Pearlescent)))
        );
        assert_eq!(
            color_finish(b" RUBBER"),
            Ok((&b""[..], Some(ColorFinish::Rubber)))
        );
        assert_eq!(
            color_finish(b" MATTE_METALLIC"),
            Ok((&b""[..], Some(ColorFinish::MatteMetallic)))
        );
        assert_eq!(
            color_finish(b" METAL"),
            Ok((&b""[..], Some(ColorFinish::Metal)))
        );
        // TODO - Should probably ensure <SPACE> or <EOF> after keyword, not *anything*
        assert_eq!(
            color_finish(b" CHROMEas"),
            Ok((&b"as"[..], Some(ColorFinish::Chrome)))
        );
        assert_eq!(
            color_finish(b" MATERIAL custom values"),
            Ok((
                &b""[..],
                Some(ColorFinish::Material(MaterialFinish::Other(
                    "custom values".to_string()
                )))
            ))
        );
    }

    #[test]
    fn test_digit1_as_u8() {
        assert_eq!(
            digit1_as_u8(b""),
            Err(nom_error(&b""[..], ErrorKind::Digit))
        );
        assert_eq!(digit1_as_u8(b"0"), Ok((&b""[..], 0u8)));
        assert_eq!(digit1_as_u8(b"1"), Ok((&b""[..], 1u8)));
        assert_eq!(digit1_as_u8(b"255"), Ok((&b""[..], 255u8)));
        assert_eq!(
            digit1_as_u8(b"256"),
            Err(nom_error(&b"256"[..], ErrorKind::MapRes))
        );
        assert_eq!(digit1_as_u8(b"32 "), Ok((&b" "[..], 32u8)));
    }

    #[test]
    fn test_meta_colour() {
        assert_eq!(meta_colour(b""), Err(nom_error(&b""[..], ErrorKind::Tag)));
        // Test one color of each type from LDCfgalt.ldr
        // The formatting is similar in LDConfig.ldr.
        assert_eq!(
            meta_colour(b"!COLOUR Black                              CODE   0   VALUE #1B2A34   EDGE #2B4354"),
            Ok((
                &b""[..],
                Command::Colour(ColourCmd {
                    name: "Black".to_string(),
                    code: 0,
                    value: Color::new(0x1B, 0x2A, 0x34),
                    edge: Color::new(0x2B, 0x43, 0x54),
                    alpha: None,
                    luminance: None,
                    finish: None
                })
            ))
        );
        assert_eq!(
            meta_colour(b"!COLOUR Trans_Dark_Blue                    CODE  33   VALUE #0020A0   EDGE #000B38   ALPHA 128"),
            Ok((
                &b""[..],
                Command::Colour(ColourCmd {
                    name: "Trans_Dark_Blue".to_string(),
                    code: 33,
                    value: Color::new(0x00, 0x20, 0xA0),
                    edge: Color::new(0x00, 0x0B, 0x38),
                    alpha: Some(128),
                    luminance: None,
                    finish: None
                })
            ))
        );
        assert_eq!(
            meta_colour(b"!COLOUR Chrome_Antique_Brass               CODE  60   VALUE #645A4C   EDGE #665B4D                               CHROME"),
            Ok((
                &b""[..],
                Command::Colour(ColourCmd {
                    name: "Chrome_Antique_Brass".to_string(),
                    code: 60,
                    value: Color::new(0x64, 0x5A, 0x4C),
                    edge: Color::new(0x66, 0x5B, 0x4D),
                    alpha: None,
                    luminance: None,
                    finish: Some(ColorFinish::Chrome)
                })
            ))
        );
        assert_eq!(
            meta_colour(b"!COLOUR Pearl_Gold                         CODE 297   VALUE #AA7F2E   EDGE #805F23                               PEARLESCENT"),
            Ok((
                &b""[..],
                Command::Colour(ColourCmd {
                    name: "Pearl_Gold".to_string(),
                    code: 297,
                    value: Color::new(0xAA, 0x7F, 0x2E),
                    edge: Color::new(0x80, 0x5F, 0x23),
                    alpha: None,
                    luminance: None,
                    finish: Some(ColorFinish::Pearlescent)
                })
            ))
        );
        assert_eq!(
            meta_colour(b"!COLOUR Metallic_Silver                    CODE  80   VALUE #767676   EDGE #8E8E8E                               METAL"),
            Ok((
                &b""[..],
                Command::Colour(ColourCmd {
                    name: "Metallic_Silver".to_string(),
                    code: 80,
                    value: Color::new(0x76, 0x76, 0x76),
                    edge: Color::new(0x8E, 0x8E, 0x8E),
                    alpha: None,
                    luminance: None,
                    finish: Some(ColorFinish::Metal)
                })
            ))
        );
        assert_eq!(
            meta_colour(b"!COLOUR Glow_In_Dark_White                 CODE 329   VALUE #F5F3D7   EDGE #E0DA85   ALPHA 240   LUMINANCE 15"),
            Ok((
                &b""[..],
                Command::Colour(ColourCmd {
                    name: "Glow_In_Dark_White".to_string(),
                    code: 329,
                    value: Color::new(0xF5, 0xF3, 0xD7),
                    edge: Color::new(0xE0, 0xDA, 0x85),
                    alpha: Some(240),
                    luminance: Some(15),
                    finish: None
                })
            ))
        );
        assert_eq!(
            meta_colour(b"!COLOUR Opal_Trans_Dark_Blue               CODE 10366 VALUE #0020A0   EDGE #000B38   ALPHA 200   LUMINANCE  5    MATERIAL GLITTER VALUE #001D38 FRACTION 0.8 VFRACTION 0.6 MINSIZE 0.02 MAXSIZE 0.1"),
            Ok((
                &b""[..],
                Command::Colour(ColourCmd {
                    name: "Opal_Trans_Dark_Blue".to_string(),
                    code: 10366,
                    value: Color::new(0x00, 0x20, 0xA0),
                    edge: Color::new(0x00, 0x0B, 0x38),
                    alpha: Some(200),
                    luminance: Some(5),
                    finish: Some(ColorFinish::Material(MaterialFinish::Glitter(
                        GlitterMaterial {
                            value: Color::new(0x00, 0x1D, 0x38),
                            alpha: None,
                            luminance: None,
                            surface_fraction: 0.8,
                            volume_fraction: 0.6,
                            size: GrainSize::MinMaxSize((0.02, 0.1)),
                        },
                    ))),
                })
            ))
        );
        assert_eq!(
            meta_colour(b"!COLOUR Speckle_Black_Silver               CODE 132   VALUE #000000   EDGE #898788                               MATERIAL SPECKLE VALUE #898788 FRACTION 0.4 MINSIZE 1 MAXSIZE 3"),
            Ok((
                &b""[..],
                Command::Colour(ColourCmd {
                    name: "Speckle_Black_Silver".to_string(),
                    code: 132,
                    value: Color::new(0x00, 0x00, 0x00),
                    edge: Color::new(0x89, 0x87, 0x88),
                    alpha: None,
                    luminance: None,
                    finish: Some(ColorFinish::Material(MaterialFinish::Speckle(
                        SpeckleMaterial {
                            value: Color::new(0x89, 0x87, 0x88),
                            alpha: None,
                            luminance: None,
                            surface_fraction: 0.4,
                            size: GrainSize::MinMaxSize((1.0, 3.0)),
                        },
                    ))),
                })
            ))
        );
        assert_eq!(
            meta_colour(b"!COLOUR Rubber_Yellow                      CODE  65   VALUE #FAC80A   EDGE #9A7C03                               RUBBER"),
            Ok((
                &b""[..],
                Command::Colour(ColourCmd {
                    name: "Rubber_Yellow".to_string(),
                    code: 65,
                    value: Color::new(0xFA, 0xC8, 0x0A),
                    edge: Color::new(0x9A, 0x7C, 0x03),
                    alpha: None,
                    luminance: None,
                    finish: Some(ColorFinish::Rubber),
                })
            ))
        );
    }

    #[test]
    fn test_vec3() {
        assert_eq!(
            read_vec3(b"0 0 0"),
            Ok((&b""[..], Vec3::new(0.0, 0.0, 0.0)))
        );
        assert_eq!(
            read_vec3(b"0 0 0 1"),
            Ok((&b" 1"[..], Vec3::new(0.0, 0.0, 0.0)))
        );
        assert_eq!(
            read_vec3(b"2 5 -7"),
            Ok((&b""[..], Vec3::new(2.0, 5.0, -7.0)))
        );
        assert_eq!(
            read_vec3(b"2.3 5 -7.4"),
            Ok((&b""[..], Vec3::new(2.3, 5.0, -7.4)))
        );
    }

    #[test]
    fn test_read_cmd_id_str() {
        assert_eq!(read_cmd_id_str(b"0"), Ok((&b""[..], &b"0"[..])));
        assert_eq!(read_cmd_id_str(b"0 "), Ok((&b""[..], &b"0"[..])));
        assert_eq!(read_cmd_id_str(b"0   "), Ok((&b""[..], &b"0"[..])));
        assert_eq!(read_cmd_id_str(b"0   e"), Ok((&b"e"[..], &b"0"[..])));
        assert_eq!(
            read_cmd_id_str(b"4547    ssd"),
            Ok((&b"ssd"[..], &b"4547"[..]))
        );
    }

    #[test]
    fn test_end_of_line() {
        assert_eq!(end_of_line(b""), Ok((&b""[..], &b""[..])));
        assert_eq!(end_of_line(b"\n"), Ok((&b""[..], &b"\n"[..])));
        assert_eq!(end_of_line(b"\r\n"), Ok((&b""[..], &b"\r\n"[..])));
    }

    #[test]
    fn test_take_not_cr_or_lf() {
        assert_eq!(take_not_cr_or_lf(b""), Ok((&b""[..], &b""[..])));
        assert_eq!(take_not_cr_or_lf(b"\n"), Ok((&b"\n"[..], &b""[..])));
        assert_eq!(take_not_cr_or_lf(b"\r\n"), Ok((&b"\r\n"[..], &b""[..])));
        assert_eq!(take_not_cr_or_lf(b"\n\n\n"), Ok((&b"\n\n\n"[..], &b""[..])));
        assert_eq!(
            take_not_cr_or_lf(b"\r\n\r\n\r\n"),
            Ok((&b"\r\n\r\n\r\n"[..], &b""[..]))
        );
        assert_eq!(take_not_cr_or_lf(b" a \n"), Ok((&b"\n"[..], &b" a "[..])));
        assert_eq!(take_not_cr_or_lf(b"test"), Ok((&b""[..], &b"test"[..])));
    }

    #[test]
    fn test_single_comma() {
        assert_eq!(single_comma(b""), Err(nom_error(&b""[..], ErrorKind::Tag)));
        assert_eq!(single_comma(b","), Ok((&b""[..], &b","[..])));
        assert_eq!(single_comma(b",s"), Ok((&b"s"[..], &b","[..])));
        assert_eq!(
            single_comma(b"w,s"),
            Err(nom_error(&b"w,s"[..], ErrorKind::Tag))
        );
    }

    #[test]
    fn test_keywords_list() {
        assert_eq!(
            keywords_list(b""),
            Err(nom_error(&b""[..], ErrorKind::TakeWhile1))
        );
        assert_eq!(keywords_list(b"a"), Ok((&b""[..], vec!["a"])));
        assert_eq!(keywords_list(b"a,b,c"), Ok((&b""[..], vec!["a", "b", "c"])));
    }

    #[test]
    fn test_filename() {
        assert_eq!(filename(b"asd\\kw/l.ldr"), Ok((&b""[..], "asd\\kw/l.ldr")));
        assert_eq!(filename(b"asdkwl.ldr"), Ok((&b""[..], "asdkwl.ldr")));
        assert_eq!(
            filename(b"asd\\kw/l.ldr\n"),
            Ok((&b"\n"[..], "asd\\kw/l.ldr"))
        );
        assert_eq!(filename(b"asdkwl.ldr\n"), Ok((&b"\n"[..], "asdkwl.ldr")));
        assert_eq!(
            filename(b"asd\\kw/l.ldr\r\n"),
            Ok((&b"\r\n"[..], "asd\\kw/l.ldr"))
        );
        assert_eq!(
            filename(b"asdkwl.ldr\r\n"),
            Ok((&b"\r\n"[..], "asdkwl.ldr"))
        );
        assert_eq!(
            filename(b"  asdkwl.ldr   \r\n"),
            Ok((&b"\r\n"[..], "asdkwl.ldr"))
        );
    }

    #[test]
    fn test_category_cmd() {
        let res = Command::Category(CategoryCmd {
            category: "Figure Accessory".to_string(),
        });
        assert_eq!(category(b"!CATEGORY Figure Accessory"), Ok((&b""[..], res)));
    }

    #[test]
    fn test_keywords_cmd() {
        let res = Command::Keywords(KeywordsCmd {
            keywords: vec![
                "western".to_string(),
                "wild west".to_string(),
                "spaghetti western".to_string(),
                "horse opera".to_string(),
                "cowboy".to_string(),
            ],
        });
        assert_eq!(
            keywords(b"!KEYWORDS western, wild west, spaghetti western, horse opera, cowboy"),
            Ok((&b""[..], res))
        );
    }

    #[test]
    fn test_comment_cmd() {
        let comment = b"test of comment, with \"weird\" characters";
        let res = Command::Comment(CommentCmd::new(std::str::from_utf8(comment).unwrap()));
        assert_eq!(meta_cmd(comment), Ok((&b""[..], res)));
        // Match empty comment too (e.g. "0" line without anything else, or "0   " with only spaces)
        assert_eq!(
            meta_cmd(b""),
            Ok((&b""[..], Command::Comment(CommentCmd::new(""))))
        );
    }

    #[test]
    fn test_file_ref_cmd() {
        let res = Command::SubFileRef(SubFileRefCmd {
            color: 16,
            pos: Vec3::new(0.0, 0.0, 0.0),
            row0: Vec3::new(1.0, 0.0, 0.0),
            row1: Vec3::new(0.0, 1.0, 0.0),
            row2: Vec3::new(0.0, 0.0, 1.0),
            file: "aaaaaaddd".to_string(),
        });
        assert_eq!(
            file_ref_cmd(b"16 0 0 0 1 0 0 0 1 0 0 0 1 aaaaaaddd"),
            Ok((&b""[..], res))
        );
    }

    #[test]
    fn test_space0() {
        assert_eq!(space0(b""), Ok((&b""[..], &b""[..])));
        assert_eq!(space0(b" "), Ok((&b""[..], &b" "[..])));
        assert_eq!(space0(b"   "), Ok((&b""[..], &b"   "[..])));
        assert_eq!(space0(b"  a"), Ok((&b"a"[..], &b"  "[..])));
        assert_eq!(space0(b"a  "), Ok((&b"a  "[..], &b""[..])));
    }

    #[test]
    fn test_space_or_eol0() {
        assert_eq!(space_or_eol0(b""), Ok((&b""[..], &b""[..])));
        assert_eq!(space_or_eol0(b" "), Ok((&b""[..], &b" "[..])));
        assert_eq!(space_or_eol0(b"   "), Ok((&b""[..], &b"   "[..])));
        assert_eq!(space_or_eol0(b"  a"), Ok((&b"a"[..], &b"  "[..])));
        assert_eq!(space_or_eol0(b"a  "), Ok((&b"a  "[..], &b""[..])));
        assert_eq!(space_or_eol0(b"\n"), Ok((&b""[..], &b"\n"[..])));
        assert_eq!(space_or_eol0(b"\n\n\n"), Ok((&b""[..], &b"\n\n\n"[..])));
        assert_eq!(space_or_eol0(b"\n\r\n"), Ok((&b""[..], &b"\n\r\n"[..])));
        // Unfortunately <LF> alone is not handled well, but we assume this needs to be ignored too
        assert_eq!(
            space_or_eol0(b"\n\r\r\r\n"),
            Ok((&b""[..], &b"\n\r\r\r\n"[..]))
        );
        assert_eq!(space_or_eol0(b"  \n"), Ok((&b""[..], &b"  \n"[..])));
        assert_eq!(space_or_eol0(b"  \n   "), Ok((&b""[..], &b"  \n   "[..])));
        assert_eq!(
            space_or_eol0(b"  \n   \r\n"),
            Ok((&b""[..], &b"  \n   \r\n"[..]))
        );
        assert_eq!(
            space_or_eol0(b"  \n   \r\n "),
            Ok((&b""[..], &b"  \n   \r\n "[..]))
        );
        assert_eq!(space_or_eol0(b"  \nsa"), Ok((&b"sa"[..], &b"  \n"[..])));
        assert_eq!(
            space_or_eol0(b"  \n  \r\nsa"),
            Ok((&b"sa"[..], &b"  \n  \r\n"[..]))
        );
    }

    #[test]
    fn test_empty_line() {
        assert_eq!(empty_line(b""), Ok((&b""[..], &b""[..])));
        assert_eq!(empty_line(b" "), Ok((&b""[..], &b" "[..])));
        assert_eq!(empty_line(b"   "), Ok((&b""[..], &b"   "[..])));
        assert_eq!(
            empty_line(b"  a"),
            Err(nom_error(&b"a"[..], ErrorKind::CrLf))
        );
        assert_eq!(
            empty_line(b"a  "),
            Err(nom_error(&b"a  "[..], ErrorKind::CrLf))
        );
    }

    #[test]
    fn test_read_cmd() {
        let res = Command::Comment(CommentCmd::new("this doesn't matter"));
        assert_eq!(read_line(b"0 this doesn't matter"), Ok((&b""[..], res)));
    }

    #[test]
    fn test_read_line_cmd() {
        let res = Command::Line(LineCmd {
            color: 16,
            vertices: [Vec3::new(1.0, 1.0, 0.0), Vec3::new(0.9239, 1.0, 0.3827)],
        });
        assert_eq!(
            read_line(b"2 16 1 1 0 0.9239 1 0.3827"),
            Ok((&b""[..], res))
        );
    }

    #[test]
    fn test_read_tri_cmd() {
        let res = Command::Triangle(TriangleCmd {
            color: 16,
            vertices: [
                Vec3::new(1.0, 1.0, 0.0),
                Vec3::new(0.9239, 1.0, 0.3827),
                Vec3::new(0.9239, 0.0, 0.3827),
            ],
            uvs: None,
        });
        assert_eq!(
            read_line(b"3 16 1 1 0 0.9239 1 0.3827 0.9239 0 0.3827"),
            Ok((&b""[..], res))
        );
        let res = Command::Triangle(TriangleCmd {
            color: 16,
            vertices: [
                Vec3::new(1.0, 1.0, 0.0),
                Vec3::new(0.9239, 1.0, 0.3827),
                Vec3::new(0.9239, 0.0, 0.3827),
            ],
            uvs: None,
        });
        assert_eq!(
            // Note: extra spaces at end
            read_line(b"3 16 1 1 0 0.9239 1 0.3827 0.9239 0 0.3827  "),
            Ok((&b""[..], res))
        );
    }

    #[test]
    fn test_read_quad_cmd() {
        let res = Command::Quad(QuadCmd {
            color: 16,
            vertices: [
                Vec3::new(1.0, 1.0, 0.0),
                Vec3::new(0.9239, 1.0, 0.3827),
                Vec3::new(0.9239, 0.0, 0.3827),
                Vec3::new(1.0, 0.0, 0.0),
            ],
            uvs: None,
        });
        assert_eq!(
            read_line(b"4 16 1 1 0 0.9239 1 0.3827 0.9239 0 0.3827 1 0 0"),
            Ok((&b""[..], res))
        );
    }

    #[test]
    fn test_read_tri_cmd_uvs() {
        let res = Command::Triangle(TriangleCmd {
            color: 16,
            vertices: [
                Vec3::new(-1.0, 0.0, 1.0),
                Vec3::new(-1.0, 0.0, -1.0),
                Vec3::new(1.0, 0.0, -1.0),
            ],
            uvs: Some([
                Vec2::new(0.0, 1.0),
                Vec2::new(0.0, 0.0),
                Vec2::new(1.0, 0.0),
            ]),
        });
        assert_eq!(
            read_line(b"3 16 -1 0 1 -1 0 -1 1 0 -1 0 1 0 0 1 0"),
            Ok((&b""[..], res))
        );
    }

    #[test]
    fn test_read_quad_cmd_uvs() {
        let res = Command::Quad(QuadCmd {
            color: 16,
            vertices: [
                Vec3::new(-1.0, 0.0, 1.0),
                Vec3::new(-1.0, 0.0, -1.0),
                Vec3::new(1.0, 0.0, -1.0),
                Vec3::new(1.0, 1.0, -1.0),
            ],
            uvs: Some([
                Vec2::new(0.0, 1.0),
                Vec2::new(0.0, 0.0),
                Vec2::new(1.0, 0.0),
                Vec2::new(1.0, 1.0),
            ]),
        });
        assert_eq!(
            read_line(b"4 16 -1 0 1 -1 0 -1 1 0 -1 1 1 -1 0 1 0 0 1 0 1 1"),
            Ok((&b""[..], res))
        );
    }

    #[test]
    fn test_read_opt_line_cmd() {
        let res = Command::OptLine(OptLineCmd {
            color: 16,
            vertices: [Vec3::new(1.0, 1.0, 0.0), Vec3::new(0.9239, 1.0, 0.3827)],
            control_points: [Vec3::new(0.9239, 0.0, 0.3827), Vec3::new(1.0, 0.0, 0.0)],
        });
        assert_eq!(
            read_line(b"5 16 1 1 0 0.9239 1 0.3827 0.9239 0 0.3827 1 0 0"),
            Ok((&b""[..], res))
        );
    }

    #[test]
    fn test_read_line_subfileref() {
        let res = Command::SubFileRef(SubFileRefCmd {
            color: 16,
            pos: Vec3::new(0.0, 0.0, 0.0),
            row0: Vec3::new(1.0, 0.0, 0.0),
            row1: Vec3::new(0.0, 1.0, 0.0),
            row2: Vec3::new(0.0, 0.0, 1.0),
            file: "aa/aaaaddd".to_string(),
        });
        assert_eq!(
            read_line(b"1 16 0 0 0 1 0 0 0 1 0 0 0 1 aa/aaaaddd"),
            Ok((&b""[..], res))
        );
    }

    #[test]
    fn test_meta_data() {
        let res = Command::Data(DataCmd {
            file: "data.bin".to_string(),
        });
        assert_eq!(read_line(b"0 !DATA data.bin"), Ok((&b""[..], res)));
    }

    #[test]
    fn test_base64_data() {
        let res = Command::Base64Data(Base64DataCmd {
            data: b"Hello World!".to_vec(),
        });
        assert_eq!(read_line(b"0 !: SGVsbG8gV29ybGQh"), Ok((&b""[..], res)));
    }

    #[test]
    fn test_file_cmd() {
        let res = Command::File(FileCmd {
            file: "submodel".to_string(),
        });
        assert_eq!(meta_cmd(b"FILE submodel"), Ok((&b""[..], res)));
    }

    #[test]
    fn test_nofile_cmd() {
        let res = Command::NoFile;
        assert_eq!(meta_cmd(b"NOFILE"), Ok((&b""[..], res)));
    }
}
