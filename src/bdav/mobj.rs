//! Module for assembling and disassembling MObj bytecode found in MovieObject.bdmv and IG button
//! navigation commands.

use super::{
    from_primitive_map_err, read_bitfield, BdavAppDetails, BdavErrorDetails, Result, SliceReader,
};
use crate::ErrorDetails;
use lalrpop_util::{lalrpop_mod, lexer::Token, ParseError};
use modular_bitfield_msb::prelude::*;
use num_derive::FromPrimitive;
use std::fmt::{Debug, Display, Formatter};
use std::io::Write;
use std::ops::Range;
use std::str::FromStr;

lalrpop_mod!(
    #[allow(clippy::all)]
    mobj,
    "/bdav/mobj.rs"
);

/// Errors that may be encountered by the MObj assembly parser.
#[derive(Debug, PartialEq)]
pub enum MObjParseErrorType {
    /// A number out of [`u32`] range was encountered.
    U32OutOfRange,
    /// A GPR register out of 0..=4095 was encountered.
    GprOutOfRange,
    /// A PSR register out of 0..=127 was encountered.
    PsrOutOfRange,
    /// `set_stream` requires audio/subtitle and ig/angle operands are both registers or both
    /// immediates. This is encountered when this constraint is violated.
    SetStreamOperandTypeMismatch,
}

/// MObj errors from the MObj assembly parser.
#[derive(Debug, PartialEq)]
pub struct MObjParseErrorDetails {
    range: Range<usize>,
    error_type: MObjParseErrorType,
}

/// Aliased [`ParseError`] that adds MObj-specific errors.
pub type MObjParseError<'a> = ParseError<usize, Token<'a>, MObjParseErrorDetails>;

/// Writes out a highlighted-text string displaying the [`MObjParseError`].
pub fn write_parse_error(
    text: &str,
    error: &MObjParseError,
    out: &mut dyn Write,
) -> std::io::Result<()> {
    struct Repeat(char, usize);

    impl Display for Repeat {
        fn fmt(&self, fmt: &mut Formatter) -> std::fmt::Result {
            for _ in 0..self.1 {
                write!(fmt, "{}", self.0)?;
            }
            Ok(())
        }
    }

    fn write_expected(out: &mut dyn Write, expected: &[String]) -> std::io::Result<()> {
        writeln!(out, "  Acceptable tokens:")?;
        for t in expected {
            writeln!(out, "  * {}", t)?;
        }
        Ok(())
    }

    let (start_col, end_col) = match error {
        ParseError::InvalidToken { location } => {
            writeln!(out, "Unexpected token encountered by parser")?;
            (*location, *location)
        }
        ParseError::UnrecognizedEOF { location, expected } => {
            writeln!(out, "Unexpected EOF encountered by parser")?;
            write_expected(out, expected)?;
            (*location, *location)
        }
        ParseError::UnrecognizedToken { token, expected } => {
            writeln!(out, "Unrecognized token encountered by parser")?;
            write_expected(out, expected)?;
            (token.0, token.2)
        }
        ParseError::ExtraToken { token } => {
            writeln!(out, "Extra tokens encountered by parser")?;
            (token.0, token.2)
        }
        ParseError::User { error } => {
            match error.error_type {
                MObjParseErrorType::U32OutOfRange => writeln!(out, "All numbers must be in u32 range")?,
                MObjParseErrorType::GprOutOfRange => writeln!(out, "GPR out of range 0..=4095")?,
                MObjParseErrorType::PsrOutOfRange => writeln!(out, "PSR out of range 0..=127")?,
                MObjParseErrorType::SetStreamOperandTypeMismatch =>
                    writeln!(out, "audio/subtitle and ig/angle operands must be both registers or both immediates")?,
            }
            (error.range.start, error.range.end)
        }
    };

    writeln!(out, "  {}", text)?;

    if end_col - start_col <= 1 {
        writeln!(out, "  {}^", Repeat(' ', start_col))?;
    } else {
        let width = end_col - start_col;
        writeln!(
            out,
            "  {}~{}~",
            Repeat(' ', start_col),
            Repeat('~', width.saturating_sub(2))
        )?;
    }

    Ok(())
}

/// MObj errors from the MObj command decoder.
#[derive(Debug)]
pub enum MObjCmdErrorDetails {
    /// Encountered an unknown [`MObjGroup`].
    UnknownMObjGroup(u8),
    /// Encountered an unknown [`BranchSubGroup`].
    UnknownBranchSubGroup(u8),
    /// Encountered an unknown [`GotoInstruction`].
    UnknownGotoInstruction(u8),
    /// Encountered an unknown [`JumpInstruction`].
    UnknownJumpInstruction(u8),
    /// Encountered an unknown [`PlayInstruction`].
    UnknownPlayInstruction(u8),
    /// Encountered an unknown [`CmpInstruction`].
    UnknownCmpInstruction(u8),
    /// Encountered an unknown [`SetSubGroup`].
    UnknownSetSubGroup(u8),
    /// Encountered an unknown [`SetInstruction`].
    UnknownSetInstruction(u8),
    /// Encountered an unknown [`SetSystemInstruction`].
    UnknownSetSystemInstruction(u8),
}

macro_rules! instruction_enum {
    // Exit rule.
    (
        @collect_unitary_variants $(#[$attr:meta])* $name:ident,
        ($(,)*) -> ($($(#[$vattr:meta])* $var:ident($str:expr) $(= $num:expr)*,)*)
    ) => {
        $(#[$attr])*
        #[repr(u8)]
        #[derive(Debug, Copy, Clone, PartialEq, FromPrimitive)]
        pub enum $name {
            $($(#[$vattr])* $var $(= $num)*,)*
        }

        impl $name {
            fn mnemonic(&self) -> &'static str {
                match self {
                    $($name::$var => $str,)*
                }
            }
        }

        impl Display for $name {
            fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.mnemonic())
            }
        }
    };

    // Handle a variant.
    (
        @collect_unitary_variants $(#[$attr:meta])* $name:ident,
        ($(#[$vattr:meta])* $var:ident($str:expr) $(= $num:expr)*, $($tail:tt)*) -> ($($var_names:tt)*)
    ) => {
        instruction_enum! {
            @collect_unitary_variants $(#[$attr])* $name,
            ($($tail)*) -> ($($var_names)* $(#[$vattr])* $var($str) $(= $num)*,)
        }
    };

    // Entry rule.
    ($(#[$attr:meta])* $name:ident { $($body:tt)* }) => {
        instruction_enum! {
            @collect_unitary_variants $(#[$attr])* $name,
            ($($body)*,) -> ()
        }
    };
}

/// Top-level MObj instruction group.
#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq, FromPrimitive)]
pub enum MObjGroup {
    /// Selects [`BranchSubGroup`].
    Branch,
    /// Selects [`CmpInstruction`].
    Cmp,
    /// Selects [`SetSubGroup`].
    Set,
}

impl Default for MObjGroup {
    fn default() -> Self {
        MObjGroup::Branch
    }
}

/// Branch instruction group.
#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq, FromPrimitive)]
pub enum BranchSubGroup {
    /// Selects [`GotoInstruction`].
    Goto,
    /// Selects [`JumpInstruction`].
    Jump,
    /// Selects [`PlayInstruction`].
    Play,
}

impl Default for BranchSubGroup {
    fn default() -> Self {
        BranchSubGroup::Goto
    }
}

instruction_enum! {
    /// Goto instructions.
    GotoInstruction {
        /// `nop`
        Nop("nop"),
        /// `goto <pc>`
        Goto("goto"),
        /// `break`
        Break("break"),
    }
}

instruction_enum! {
    /// Jump instructions.
    JumpInstruction {
        /// `jump_object <id>`
        JumpObject("jump_object"),
        /// `jump_title <id>`
        JumpTitle("jump_title"),
        /// `call_object <id>`
        CallObject("call_object"),
        /// `call_title <id>`
        CallTitle("call_title"),
        /// `resume`
        Resume("resume"),
    }
}

instruction_enum! {
    /// Play instructions.
    PlayInstruction {
        /// `play_pl <id>`
        PlayPlaylist("play_pl"),
        /// `play_pl_pi <id> <id>`
        PlayPlaylistItem("play_pl_pi"),
        /// `play_pl_pm <id> <id>`
        PlayPlaylistMark("play_pl_pm"),
        /// `terminate_pl`
        TerminatePlaylist("terminate_pl"),
        /// `link_pi <id>`
        LinkItem("link_pi"),
        /// `link_mk <id>`
        LinkMark("link_mk"),
    }
}

instruction_enum! {
    /// Cmp instructions.
    CmpInstruction {
        /// `bc <a> <b>`
        Bc("bc") = 0x1,
        /// `eq <a> <b>`
        Eq("eq"),
        /// `ne <a> <b>`
        Ne("ne"),
        /// `ge <a> <b>`
        Ge("ge"),
        /// `gt <a> <b>`
        Gt("gt"),
        /// `le <a> <b>`
        Le("le"),
        /// `lt <a> <b>`
        Lt("lt"),
    }
}

/// Set instruction group.
#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq, FromPrimitive)]
pub enum SetSubGroup {
    /// Selects [`SetInstruction`].
    Set,
    /// Selects [`SetSystemInstruction`].
    SetSystem,
}

impl Default for SetSubGroup {
    fn default() -> Self {
        SetSubGroup::Set
    }
}

instruction_enum! {
    /// Set instructions.
    SetInstruction {
        /// `move <a> <b>`
        Move("move") = 0x1,
        /// `swap <a> <b>`
        Swap("swap"),
        /// `add <a> <b>`
        Add("add"),
        /// `sub <a> <b>`
        Sub("sub"),
        /// `mul <a> <b>`
        Mul("mul"),
        /// `div <a> <b>`
        Div("div"),
        /// `mod <a> <b>`
        Mod("mod"),
        /// `rnd <a> <b>`
        Rnd("rnd"),
        /// `and <a> <b>`
        And("and"),
        /// `or <a> <b>`
        Or("or"),
        /// `xor <a> <b>`
        Xor("xor"),
        /// `bset <a> <b>`
        Bitset("bset"),
        /// `bclr <a> <b>`
        Bitclr("bclr"),
        /// `shl <a> <b>`
        Shl("shl"),
        /// `shr <a> <b>`
        Shr("shr"),
    }
}

instruction_enum! {
    /// SetSystem instructions.
    SetSystemInstruction {
        /// `set_stream <audio-id>, <subtitle-id>, <subtitle "enabled"|"disabled">, <ig-id>, <angle-id>`
        SetStream("set_stream") = 0x1,
        /// `set_nv_timer <a> <b>`
        SetNvTimer("set_nv_timer"),
        /// `set_nv_timer <button-id> <page-id>`
        SetButtonPage("set_button_page"),
        /// `enable_button <button-id>`
        EnableButton("enable_button"),
        /// `disable_button <button-id>`
        DisableButton("disable_button"),
        /// `set_sec_stream <a> <b>`
        SetSecStream("set_sec_stream"),
        /// `popup_off`
        PopupOff("popup_off"),
        /// `still_on`
        StillOn("still_on"),
        /// `still_off`
        StillOff("still_off"),
        /// `set_output_mode <a>`
        SetOutputMode("set_output_mode"),
        /// `set_stream_ss <audio-id>, <subtitle-id>, <subtitle "enabled"|"disabled">, <ig-id>, <angle-id>`
        SetStreamSs("set_stream_ss"),
        /// `bd_plus_msg <a> <b>`
        BdPlusMsg("bd_plus_msg") = 0x10,
    }
}

/// Operation information of one [`MObjCmd`]
#[bitfield]
#[derive(Debug)]
pub struct MObjInstruction {
    pub op_cnt: B3,
    pub grp: B2,
    pub sub_grp: B3,
    pub imm_op1: bool,
    pub imm_op2: bool,
    #[skip]
    pub padding: B2,
    pub branch_opt: B4,
    #[skip]
    pub padding2: B4,
    pub cmp_opt: B4,
    #[skip]
    pub padding3: B3,
    pub set_opt: B5,
}

/// A command in the MObj VM.
pub struct MObjCmd {
    /// Operation information.
    pub inst: MObjInstruction,
    /// Dst operand.
    pub dst: u32,
    /// Src operand.
    pub src: u32,
}

impl MObjCmd {
    /// Parses 12 bytes of command bytecode.
    pub fn parse<D: BdavAppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let inst = read_bitfield!(reader, MObjInstruction);
        let dst = reader.read_be_u32()?;
        let src = reader.read_be_u32()?;
        let new_cmd = Self { inst, dst, src };
        new_cmd.validate().map_err(|e| {
            reader.make_error(ErrorDetails::AppError(BdavErrorDetails::BadMObjCommand(e)))
        })?;
        Ok(new_cmd)
    }

    /// Assembles a command from an assembly string.
    pub fn assemble(s: &str) -> std::result::Result<Self, MObjParseError> {
        mobj::CmdParser::new().parse(s)
    }

    /// Visit instruction with command category resolved.
    pub fn visit<V: MObjCmdVisitor<R>, R>(
        &self,
        visitor: V,
    ) -> std::result::Result<R, MObjCmdErrorDetails> {
        Ok(
            match from_primitive_map_err(self.inst.grp(), |v| {
                MObjCmdErrorDetails::UnknownMObjGroup(v)
            })? {
                MObjGroup::Branch => match from_primitive_map_err(self.inst.sub_grp(), |v| {
                    MObjCmdErrorDetails::UnknownBranchSubGroup(v)
                })? {
                    BranchSubGroup::Goto => visitor
                        .visit_goto(from_primitive_map_err(self.inst.branch_opt(), |v| {
                            MObjCmdErrorDetails::UnknownGotoInstruction(v)
                        })?),
                    BranchSubGroup::Jump => visitor
                        .visit_jump(from_primitive_map_err(self.inst.branch_opt(), |v| {
                            MObjCmdErrorDetails::UnknownJumpInstruction(v)
                        })?),
                    BranchSubGroup::Play => visitor
                        .visit_play(from_primitive_map_err(self.inst.branch_opt(), |v| {
                            MObjCmdErrorDetails::UnknownPlayInstruction(v)
                        })?),
                },
                MObjGroup::Cmp => visitor
                    .visit_cmp(from_primitive_map_err(self.inst.cmp_opt(), |v| {
                        MObjCmdErrorDetails::UnknownCmpInstruction(v)
                    })?),
                MObjGroup::Set => match from_primitive_map_err(self.inst.sub_grp(), |v| {
                    MObjCmdErrorDetails::UnknownSetSubGroup(v)
                })? {
                    SetSubGroup::Set => visitor
                        .visit_set(from_primitive_map_err(self.inst.set_opt(), |v| {
                            MObjCmdErrorDetails::UnknownSetInstruction(v)
                        })?),
                    SetSubGroup::SetSystem => visitor
                        .visit_set_system(from_primitive_map_err(self.inst.set_opt(), |v| {
                            MObjCmdErrorDetails::UnknownSetSystemInstruction(v)
                        })?),
                },
            },
        )
    }

    /// Ensures a valid command hierarchy is present.
    pub fn validate(&self) -> std::result::Result<(), MObjCmdErrorDetails> {
        self.visit(CmdValidate)
    }

    /// Gets string of command mnemonic.
    pub fn mnemonic(&self) -> &'static str {
        match self.visit(GetCmdMnemonic) {
            Ok(s) => s,
            Err(ed) => match ed {
                MObjCmdErrorDetails::UnknownMObjGroup(u8) => "<BAD MOBJ GROUP>",
                MObjCmdErrorDetails::UnknownBranchSubGroup(u8) => "<BAD BRANCH SUBGROUP>",
                MObjCmdErrorDetails::UnknownGotoInstruction(u8) => "<BAD GOTO INSTRUCTION>",
                MObjCmdErrorDetails::UnknownJumpInstruction(u8) => "<BAD JUMP INSTRUCTION>",
                MObjCmdErrorDetails::UnknownPlayInstruction(u8) => "<BAD PLAY INSTRUCTION>",
                MObjCmdErrorDetails::UnknownCmpInstruction(u8) => "<BAD CMP INSTRUCTION>",
                MObjCmdErrorDetails::UnknownSetSubGroup(u8) => "<BAD SET SUBGROUP>",
                MObjCmdErrorDetails::UnknownSetInstruction(u8) => "<BAD SET INSTRUCTION>",
                MObjCmdErrorDetails::UnknownSetSystemInstruction(u8) => {
                    "<BAD SETSYSTEM INSTRUCTION>"
                }
            },
        }
    }

    fn make_operand(v: u32, is_imm: bool) -> MObjOperand {
        if is_imm {
            MObjOperand::Imm(v)
        } else if v & 0x80000000 == 0 {
            MObjOperand::Gpr(v & 0xfff)
        } else {
            MObjOperand::Psr(v & 0x7f)
        }
    }

    fn dst_operand(&self) -> MObjOperand {
        Self::make_operand(self.dst, self.inst.imm_op1())
    }

    fn src_operand(&self) -> MObjOperand {
        Self::make_operand(self.src, self.inst.imm_op2())
    }
}

macro_rules! format_cmd {
    ($fmt_type:ident) => {
        impl $fmt_type for MObjCmd {
            fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
                if let MObjGroup::Set =
                    from_primitive_map_err(self.inst.grp(), |_| std::fmt::Error)?
                {
                    let sub_grp: SetSubGroup =
                        from_primitive_map_err(self.inst.sub_grp(), |_| std::fmt::Error)?;
                    if sub_grp == SetSubGroup::SetSystem {
                        let inst: SetSystemInstruction =
                            from_primitive_map_err(self.inst.set_opt(), |_| std::fmt::Error)?;
                        match inst {
                            // TODO: Operands of SetStreamSs not known
                            SetSystemInstruction::SetStream | SetSystemInstruction::SetStreamSs => {
                                let primary_audio_flag = (self.dst >> 28) & 0x8 != 0;
                                let primary_audio_id = Self::make_operand(
                                    (self.dst & 0x0fff0000) >> 16,
                                    self.inst.imm_op1(),
                                );
                                let pg_text_st_flag = ((self.dst & 0xf000) >> 12) & 0x8 != 0;
                                let pg_text_st_enabled = ((self.dst & 0xf000) >> 12) & 0x4 != 0;
                                let pg_text_st_id =
                                    Self::make_operand(self.dst & 0xfff, self.inst.imm_op1());

                                let ig_flag = (self.src >> 28) & 0x8 != 0;
                                let ig_id = Self::make_operand(
                                    (self.src & 0x0fff0000) >> 16,
                                    self.inst.imm_op2(),
                                );
                                let angle_flag = ((self.src & 0xf000) >> 12) & 0x8 != 0;
                                let angle_id =
                                    Self::make_operand(self.src & 0xfff, self.inst.imm_op2());

                                f.write_str(self.mnemonic())?;
                                f.write_str(" ")?;
                                if primary_audio_flag {
                                    $fmt_type::fmt(&primary_audio_id, f)?;
                                } else {
                                    f.write_str("none")?;
                                }
                                f.write_str(", ")?;
                                if pg_text_st_flag {
                                    $fmt_type::fmt(&pg_text_st_id, f)?;
                                } else {
                                    f.write_str("none")?;
                                }
                                f.write_str(", ")?;
                                if pg_text_st_enabled {
                                    f.write_str("enabled")?;
                                } else {
                                    f.write_str("disabled")?;
                                }
                                f.write_str(", ")?;
                                if ig_flag {
                                    $fmt_type::fmt(&ig_id, f)?;
                                } else {
                                    f.write_str("none")?;
                                }
                                f.write_str(", ")?;
                                if angle_flag {
                                    $fmt_type::fmt(&angle_id, f)?;
                                } else {
                                    f.write_str("none")?;
                                }
                                return Ok(());
                            }
                            SetSystemInstruction::SetButtonPage => {
                                let button_flag = self.dst & 0x80000000 != 0;
                                let button_id =
                                    Self::make_operand(self.dst & 0x3fffffff, self.inst.imm_op1());
                                let page_flag = self.src & 0x80000000 != 0;
                                let effect_flag = self.src & 0x40000000 != 0;
                                let page_id =
                                    Self::make_operand(self.src & 0x3fffffff, self.inst.imm_op2());

                                f.write_str(self.mnemonic())?;
                                f.write_str(" ")?;
                                if button_flag {
                                    $fmt_type::fmt(&button_id, f)?;
                                } else {
                                    f.write_str("none")?;
                                }
                                f.write_str(", ")?;
                                if page_flag {
                                    $fmt_type::fmt(&page_id, f)?;
                                } else {
                                    f.write_str("none")?;
                                }
                                if effect_flag {
                                    f.write_str(", skip_out")?;
                                }
                                return Ok(());
                            }
                            _ => {}
                        }
                    }
                }

                match self.inst.op_cnt() {
                    0 => f.write_str(self.mnemonic()),
                    1 => {
                        f.write_str(self.mnemonic())?;
                        f.write_str(" ")?;
                        $fmt_type::fmt(&self.dst_operand(), f)
                    }
                    _ => {
                        f.write_str(self.mnemonic())?;
                        f.write_str(" ")?;
                        $fmt_type::fmt(&self.dst_operand(), f)?;
                        f.write_str(", ")?;
                        $fmt_type::fmt(&self.src_operand(), f)
                    }
                }
            }
        }
    };
}

format_cmd!(Display);
format_cmd!(Debug);

/// Visitor for each MObj command category. Use with [`MObjCmd::visit`].
pub trait MObjCmdVisitor<R> {
    /// Called when command contains a [`GotoInstruction`].
    fn visit_goto(self, inst: GotoInstruction) -> R;
    /// Called when command contains a [`JumpInstruction`].
    fn visit_jump(self, inst: JumpInstruction) -> R;
    /// Called when command contains a [`PlayInstruction`].
    fn visit_play(self, inst: PlayInstruction) -> R;
    /// Called when command contains a [`CmpInstruction`].
    fn visit_cmp(self, inst: CmpInstruction) -> R;
    /// Called when command contains a [`SetInstruction`].
    fn visit_set(self, inst: SetInstruction) -> R;
    /// Called when command contains a [`SetSystemInstruction`].
    fn visit_set_system(self, inst: SetSystemInstruction) -> R;
}

struct CmdValidate;

impl MObjCmdVisitor<()> for CmdValidate {
    fn visit_goto(self, inst: GotoInstruction) {}
    fn visit_jump(self, inst: JumpInstruction) {}
    fn visit_play(self, inst: PlayInstruction) {}
    fn visit_cmp(self, inst: CmpInstruction) {}
    fn visit_set(self, inst: SetInstruction) {}
    fn visit_set_system(self, inst: SetSystemInstruction) {}
}

struct GetCmdMnemonic;

impl MObjCmdVisitor<&'static str> for GetCmdMnemonic {
    fn visit_goto(self, inst: GotoInstruction) -> &'static str {
        inst.mnemonic()
    }
    fn visit_jump(self, inst: JumpInstruction) -> &'static str {
        inst.mnemonic()
    }
    fn visit_play(self, inst: PlayInstruction) -> &'static str {
        inst.mnemonic()
    }
    fn visit_cmp(self, inst: CmpInstruction) -> &'static str {
        inst.mnemonic()
    }
    fn visit_set(self, inst: SetInstruction) -> &'static str {
        inst.mnemonic()
    }
    fn visit_set_system(self, inst: SetSystemInstruction) -> &'static str {
        inst.mnemonic()
    }
}

#[derive(PartialEq, Copy, Clone)]
pub(crate) enum MObjOperand {
    Gpr(u32),
    Psr(u32),
    Imm(u32),
}

impl MObjOperand {
    fn into_val(self) -> u32 {
        match self {
            MObjOperand::Gpr(v) => v,
            MObjOperand::Psr(v) => 0x80000000 | v,
            MObjOperand::Imm(v) => v,
        }
    }

    fn is_imm(&self) -> bool {
        matches!(self, MObjOperand::Imm(_))
    }

    fn psr_comment(&self) -> &'static str {
        match self {
            MObjOperand::Psr(v) => match v {
                0 => "/* Interactive graphics stream number */",
                1 => "/* Primary audio stream number */",
                2 => "/* PG TextST stream number and PiP PG stream number */",
                3 => "/* Angle number */",
                4 => "/* Title number */",
                5 => "/* Chapter number */",
                6 => "/* PlayList ID */",
                7 => "/* PlayItem ID */",
                8 => "/* Presentation time */",
                9 => "/* Navigation timer */",
                10 => "/* Selected button ID */",
                11 => "/* Page ID */",
                12 => "/* User style number */",
                13 => "/* RO: User age */",
                14 => "/* Secondary audio stream number and secondary video stream number */",
                15 => "/* RO: player capability for audio */",
                16 => "/* RO: Language code for audio */",
                17 => "/* RO: Language code for PG and Text subtitles */",
                18 => "/* RO: Menu description language code */",
                19 => "/* RO: Country code */",
                20 => "/* RO: Region code */ /* 1 - A, 2 - B, 4 - C */",
                21 => "/* RO: Output Mode Preference */ /* 0 - 2D, 1 - 3D */",
                22 => "/* Stereoscopic status */ /* 2D / 3D */ ",
                23 => "/* RO: display capability */",
                24 => "/* RO: 3D capability */",
                25 => "/* RO: UHD capability */",
                26 => "/* RO: UHD display capability */",
                27 => "/* RO: HDR preference */",
                28 => "/* RO: SDR conversion preference */",
                29 => "/* RO: player capability for video */",
                30 => "/* RO: player capability for text subtitle */",
                31 => "/* RO: Player profile and version */",
                36 => "/* backup PSR4 */",
                37 => "/* backup PSR5 */",
                38 => "/* backup PSR6 */",
                39 => "/* backup PSR7 */",
                40 => "/* backup PSR8 */",
                42 => "/* backup PSR10 */",
                43 => "/* backup PSR11 */",
                44 => "/* backup PSR12 */",
                48 => "/* RO: Characteristic text caps */",
                49 => "/* RO: Characteristic text caps */",
                50 => "/* RO: Characteristic text caps */",
                51 => "/* RO: Characteristic text caps */",
                52 => "/* RO: Characteristic text caps */",
                53 => "/* RO: Characteristic text caps */",
                54 => "/* RO: Characteristic text caps */",
                55 => "/* RO: Characteristic text caps */",
                56 => "/* RO: Characteristic text caps */",
                57 => "/* RO: Characteristic text caps */",
                58 => "/* RO: Characteristic text caps */",
                59 => "/* RO: Characteristic text caps */",
                60 => "/* RO: Characteristic text caps */",
                61 => "/* RO: Characteristic text caps */",
                102 => "/* BD+ receive */",
                103 => "/* BD+ send */",
                104 => "/* BD+ shared */",
                _ => "",
            },
            _ => "",
        }
    }
}

impl Display for MObjOperand {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            MObjOperand::Gpr(v) => {
                f.write_str("r")?;
                Display::fmt(&v, f)
            }
            MObjOperand::Psr(v) => {
                f.write_str("PSR")?;
                Display::fmt(&v, f)
            }
            MObjOperand::Imm(v) => Display::fmt(&v, f),
        }
    }
}

impl Debug for MObjOperand {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            MObjOperand::Gpr(v) => {
                f.write_str("r")?;
                Debug::fmt(&v, f)
            }
            MObjOperand::Psr(v) => {
                f.write_str("PSR")?;
                Debug::fmt(&v, f)?;
                let comment = self.psr_comment();
                if !comment.is_empty() {
                    f.write_str(" ")?;
                    f.write_str(comment)?;
                }
                Ok(())
            }
            MObjOperand::Imm(v) => Debug::fmt(&v, f),
        }
    }
}

fn check_set_stream_operands<'a>(
    range: Range<usize>,
    op1: &Option<MObjOperand>,
    op2: &Option<MObjOperand>,
) -> std::result::Result<(), MObjParseError<'a>> {
    if let (Some(op1), Some(op2)) = (op1, op2) {
        if op1.is_imm() != op2.is_imm() {
            return Err(ParseError::User {
                error: MObjParseErrorDetails {
                    range,
                    error_type: MObjParseErrorType::SetStreamOperandTypeMismatch,
                },
            });
        }
    }
    Ok(())
}

fn is_optional_operand_imm(op: &Option<MObjOperand>) -> bool {
    if let Some(op) = op {
        op.is_imm()
    } else {
        false
    }
}

fn set_stream_operand_to_val(op: &Option<MObjOperand>) -> u32 {
    if let Some(op) = op {
        0x8000 | ((*op).into_val() & 0xfff)
    } else {
        0x0
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn make_set_stream_cmd<'a>(
    instruction: SetSystemInstruction,
    range1: Range<usize>,
    primary_audio: Option<MObjOperand>,
    pg_text_st: Option<MObjOperand>,
    pg_text_st_enabled: bool,
    range2: Range<usize>,
    ig: Option<MObjOperand>,
    angle: Option<MObjOperand>,
) -> std::result::Result<MObjCmd, MObjParseError<'a>> {
    assert!(
        instruction == SetSystemInstruction::SetStream
            || instruction == SetSystemInstruction::SetStreamSs
    );
    check_set_stream_operands(range1, &primary_audio, &pg_text_st)?;
    check_set_stream_operands(range2, &ig, &angle)?;

    let primary_audio_val = set_stream_operand_to_val(&primary_audio);
    let pg_text_st_val = set_stream_operand_to_val(&pg_text_st);
    let dst_val =
        primary_audio_val << 16 | pg_text_st_val | if pg_text_st_enabled { 0x4000 } else { 0x0 };

    let ig_val = set_stream_operand_to_val(&ig);
    let angle_val = set_stream_operand_to_val(&angle);
    let src_val = ig_val << 16 | angle_val;

    Ok(MObjCmd {
        inst: MObjInstruction::new()
            .with_op_cnt(2)
            .with_grp(MObjGroup::Set as u8)
            .with_sub_grp(SetSubGroup::SetSystem as u8)
            .with_imm_op1(is_optional_operand_imm(&primary_audio))
            .with_imm_op2(is_optional_operand_imm(&ig))
            .with_set_opt(instruction as u8),
        dst: dst_val,
        src: src_val,
    })
}

fn set_button_page_operand_to_val(op: &Option<MObjOperand>) -> u32 {
    if let Some(op) = op {
        0x80000000 | ((*op).into_val() & 0x3fffffff)
    } else {
        0x0
    }
}

pub(crate) fn make_set_button_page_cmd<'a>(
    button: Option<MObjOperand>,
    page: Option<MObjOperand>,
    skip_out: bool,
) -> std::result::Result<MObjCmd, MObjParseError<'a>> {
    let dst_val = set_button_page_operand_to_val(&button);
    let src_val = set_button_page_operand_to_val(&page) | if skip_out { 0x40000000 } else { 0x0 };

    Ok(MObjCmd {
        inst: MObjInstruction::new()
            .with_op_cnt(2)
            .with_grp(MObjGroup::Set as u8)
            .with_sub_grp(SetSubGroup::SetSystem as u8)
            .with_imm_op1(is_optional_operand_imm(&button))
            .with_imm_op2(is_optional_operand_imm(&page))
            .with_set_opt(SetSystemInstruction::SetButtonPage as u8),
        dst: dst_val,
        src: src_val,
    })
}

fn assemble_cmd(s: &str) -> String {
    MObjCmd::assemble(s).unwrap().to_string()
}

fn test_cmd(s: &str) {
    assert_eq!(assemble_cmd(s), s);
}

#[test]
fn test_assemble_operands() {
    test_cmd("goto 1");
    assert_eq!(assemble_cmd("goto /* some comment */ 1"), "goto 1");
    test_cmd("goto r1");
    test_cmd("goto PSR1");
    test_cmd("goto PSR127");
    assert_eq!(
        MObjCmd::assemble("goto PSR128").unwrap_err(),
        MObjParseError::User {
            error: MObjParseErrorDetails {
                range: 8..11,
                error_type: MObjParseErrorType::PsrOutOfRange
            }
        }
    );
    test_cmd("goto r4095");
    assert_eq!(
        MObjCmd::assemble("goto r4096").unwrap_err(),
        MObjParseError::User {
            error: MObjParseErrorDetails {
                range: 6..10,
                error_type: MObjParseErrorType::GprOutOfRange
            }
        }
    );
    assert_eq!(
        MObjCmd::assemble("goto 999999999999").unwrap_err(),
        MObjParseError::User {
            error: MObjParseErrorDetails {
                range: 5..17,
                error_type: MObjParseErrorType::U32OutOfRange
            }
        }
    );
    assert_eq!(
        MObjCmd::assemble("goto -1").unwrap_err(),
        MObjParseError::InvalidToken { location: 5 }
    );
    assert_eq!(assemble_cmd("goto 0x10"), "goto 16");
    assert_eq!(assemble_cmd("goto r0x10"), "goto r16");
    assert_eq!(assemble_cmd("goto PSR0x10"), "goto PSR16");

    test_cmd("set_stream r1, r2, enabled, r3, r4");
    test_cmd("set_stream 1, 2, enabled, r3, r4");
    test_cmd("set_stream r1, r2, enabled, 3, 4");
    test_cmd("set_stream 1, 2, enabled, 3, 4");
    assert_eq!(
        MObjCmd::assemble("set_stream r1, 2, enabled, r3, r4").unwrap_err(),
        MObjParseError::User {
            error: MObjParseErrorDetails {
                range: 11..16,
                error_type: MObjParseErrorType::SetStreamOperandTypeMismatch
            }
        }
    );

    test_cmd("set_button_page r1, r2");
    test_cmd("set_button_page 1, r2");
    test_cmd("set_button_page r1, 2");
    test_cmd("set_button_page 1, 2");
    test_cmd("set_button_page r1, r2, skip_out");
}

#[test]
fn test_assemble_cmds() {
    test_cmd("nop");
    test_cmd("goto r1");
    test_cmd("break");

    test_cmd("jump_object r1");
    test_cmd("jump_title r1");
    test_cmd("call_object r1");
    test_cmd("call_title r1");
    test_cmd("resume");

    test_cmd("play_pl r1");
    test_cmd("play_pl_pi r1, r2");
    test_cmd("play_pl_pm r1, r2");
    test_cmd("terminate_pl");
    test_cmd("link_pi r1");
    test_cmd("link_mk r1");

    test_cmd("bc r1, r2");
    test_cmd("eq r1, r2");
    test_cmd("ne r1, r2");
    test_cmd("ge r1, r2");
    test_cmd("gt r1, r2");
    test_cmd("le r1, r2");
    test_cmd("lt r1, r2");

    test_cmd("move r1, r2");
    test_cmd("swap r1, r2");
    test_cmd("add r1, r2");
    test_cmd("sub r1, r2");
    test_cmd("mul r1, r2");
    test_cmd("div r1, r2");
    test_cmd("mod r1, r2");
    test_cmd("rnd r1, r2");
    test_cmd("and r1, r2");
    test_cmd("or r1, r2");
    test_cmd("xor r1, r2");
    test_cmd("bset r1, r2");
    test_cmd("bclr r1, r2");
    test_cmd("shl r1, r2");
    test_cmd("shr r1, r2");

    test_cmd("set_stream r1, r2, enabled, r3, r4");
    test_cmd("set_nv_timer r1, r2");
    test_cmd("set_button_page r1, r2");
    test_cmd("enable_button r1");
    test_cmd("disable_button r1");
    test_cmd("set_sec_stream r1, r2");
    test_cmd("popup_off");
    test_cmd("still_on");
    test_cmd("still_off");
    test_cmd("set_output_mode r1");
    test_cmd("set_stream_ss r1, r2, enabled, r3, r4");
    test_cmd("bd_plus_msg r1, r2");
}
