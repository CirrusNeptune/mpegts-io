use super::{read_bitfield, AppDetails, Result, SliceReader};
use lalrpop_util::{lalrpop_mod, lexer::Token, ParseError};
use modular_bitfield_msb::prelude::*;
use num_derive::FromPrimitive;
use num_traits::FromPrimitive;
use std::fmt::{Debug, Display, Formatter};
use std::str::FromStr;

lalrpop_mod!(
    #[allow(clippy::all)]
    mobj,
    "/bdav/mobj.rs"
);

#[derive(Debug, PartialEq)]
pub enum MObjParseErrorDetails {
    GprOutOfRange,
    PsrOutOfRange,
}

pub type MObjParseError<'a> = ParseError<usize, Token<'a>, MObjParseErrorDetails>;

#[derive(Debug)]
pub enum MObjError {
    InstructionDoesNotExist(String),
}

#[repr(u8)]
#[derive(Debug, BitfieldSpecifier)]
#[bits = 2]
pub(crate) enum MObjGroup {
    Branch,
    Cmp,
    Set,
}

#[derive(Debug, Copy, Clone, PartialEq, FromPrimitive)]
pub(crate) enum BranchSubGroup {
    Goto,
    Jump,
    Play,
}

macro_rules! instruction_enum {
    // Exit rule.
    (
        @collect_unitary_variants $name:ident,
        ($(,)*) -> ($($var:ident($str:expr) $(= $num:expr)*,)*)
    ) => {
        #[derive(Debug, Copy, Clone, PartialEq, FromPrimitive)]
        pub(crate) enum $name {
            $($var $(= $num)*,)*
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

        impl FromStr for $name {
            type Err = MObjError;

            fn from_str(s: &str) -> std::result::Result<Self, MObjError> {
                let upper_s = s.to_lowercase();
                match upper_s.as_str() {
                    $($str => Ok($name::$var),)*
                    _ => Err(MObjError::InstructionDoesNotExist(upper_s)),
                }
            }
        }
    };

    // Handle a variant.
    (
        @collect_unitary_variants $name:ident,
        ($var:ident($str:expr) $(= $num:expr)*, $($tail:tt)*) -> ($($var_names:tt)*)
    ) => {
        instruction_enum! {
            @collect_unitary_variants $name,
            ($($tail)*) -> ($($var_names)* $var($str) $(= $num)*,)
        }
    };

    // Entry rule.
    ($name:ident { $($body:tt)* }) => {
        instruction_enum! {
            @collect_unitary_variants $name,
            ($($body)*,) -> ()
        }
    };
}

instruction_enum! {
    GotoInstruction {
        Nop("nop"),
        Goto("goto"),
        Break("break"),
    }
}

instruction_enum! {
    JumpInstruction {
        JumpObject("jump_object"),
        JumpTitle("jump_title"),
        CallObject("call_object"),
        CallTitle("call_title"),
        Resume("resume"),
    }
}

instruction_enum! {
    PlayInstruction {
        PlayPlaylist("play_pl"),
        PlayPlaylistItem("play_pl_pi"),
        PlayPlaylistMark("play_pl_pm"),
        TerminatePlaylist("terminate_pl"),
        LinkItem("link_pi"),
        LinkMark("link_mk"),
    }
}

instruction_enum! {
    CmpInstruction {
        Bc("bc") = 0x1,
        Eq("eq"),
        Ne("ne"),
        Ge("ge"),
        Gt("gt"),
        Le("le"),
        Lt("lt"),
    }
}

#[derive(Debug, Copy, Clone, PartialEq, FromPrimitive)]
pub(crate) enum SetSubGroup {
    Set,
    SetSystem,
}

instruction_enum! {
    SetInstruction {
        Move("move") = 0x1,
        Swap("swap"),
        Add("add"),
        Sub("sub"),
        Mul("mul"),
        Div("div"),
        Mod("mod"),
        Rnd("rnd"),
        And("and"),
        Or("or"),
        Xor("xor"),
        Bitset("bset"),
        Bitclr("bclr"),
        Shl("shl"),
        Shr("shr"),
    }
}

instruction_enum! {
    SetSystemInstruction {
        SetStream("set_stream") = 0x1,
        SetNvTimer("set_nv_timer"),
        SetButtonPage("set_button_page"),
        EnableButton("enable_button"),
        DisableButton("disable_button"),
        SetSecStream("set_sec_stream"),
        PopupOff("popup_off"),
        StillOn("still_on"),
        StillOff("still_off"),
        SetOutputMode("set_output_mode"),
        SetStreamSs("set_stream_ss"),
        BdPlusMsg("bd_plus_msg") = 0x10,
    }
}

#[bitfield]
#[derive(Debug)]
pub(crate) struct MObjInstruction {
    pub op_cnt: B3,
    pub grp: MObjGroup,
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

pub(crate) struct MObjCmd {
    pub inst: MObjInstruction,
    pub dst: u32,
    pub src: u32,
}

impl MObjCmd {
    pub(crate) fn parse<D: AppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let inst = read_bitfield!(reader, MObjInstruction);
        let dst = reader.read_be_u32()?;
        let src = reader.read_be_u32()?;
        Ok(Self { inst, dst, src })
    }

    pub(crate) fn assemble(s: &str) -> std::result::Result<Self, MObjParseError> {
        mobj::CmdParser::new().parse(s)
    }

    fn visit<V: MObjCmdVisitor<R>, R>(&self, visitor: V) -> R {
        match self.inst.grp() {
            MObjGroup::Branch => match FromPrimitive::from_u8(self.inst.sub_grp()).unwrap() {
                BranchSubGroup::Goto => {
                    visitor.visit_goto(FromPrimitive::from_u8(self.inst.branch_opt()).unwrap())
                }
                BranchSubGroup::Jump => {
                    visitor.visit_jump(FromPrimitive::from_u8(self.inst.branch_opt()).unwrap())
                }
                BranchSubGroup::Play => {
                    visitor.visit_play(FromPrimitive::from_u8(self.inst.branch_opt()).unwrap())
                }
            },
            MObjGroup::Cmp => {
                visitor.visit_cmp(FromPrimitive::from_u8(self.inst.cmp_opt()).unwrap())
            }
            MObjGroup::Set => match FromPrimitive::from_u8(self.inst.sub_grp()).unwrap() {
                SetSubGroup::Set => {
                    visitor.visit_set(FromPrimitive::from_u8(self.inst.set_opt()).unwrap())
                }
                SetSubGroup::SetSystem => {
                    visitor.visit_set_system(FromPrimitive::from_u8(self.inst.set_opt()).unwrap())
                }
            },
        }
    }

    fn mnemonic(&self) -> &'static str {
        self.visit(GetCmdMnemonic)
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
                if let MObjGroup::Set = self.inst.grp() {
                    let sub_grp: SetSubGroup = FromPrimitive::from_u8(self.inst.sub_grp()).unwrap();
                    if sub_grp == SetSubGroup::SetSystem {
                        let inst: SetSystemInstruction =
                            FromPrimitive::from_u8(self.inst.set_opt()).unwrap();
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
                                    f.write_str("text_st_enabled")?;
                                } else {
                                    f.write_str("text_st_disabled")?;
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

trait MObjCmdVisitor<R> {
    fn visit_goto(self, inst: GotoInstruction) -> R;
    fn visit_jump(self, inst: JumpInstruction) -> R;
    fn visit_play(self, inst: PlayInstruction) -> R;
    fn visit_cmp(self, inst: CmpInstruction) -> R;
    fn visit_set(self, inst: SetInstruction) -> R;
    fn visit_set_system(self, inst: SetSystemInstruction) -> R;
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

#[derive(PartialEq)]
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

fn assemble_cmd(s: &str) -> String {
    MObjCmd::assemble(s).unwrap().to_string()
}

#[test]
fn test_assemble_operands() {
    assert_eq!(assemble_cmd("goto 1"), "goto 1");
    assert_eq!(assemble_cmd("goto /* some comment */ 1"), "goto 1");
    assert_eq!(assemble_cmd("goto r1"), "goto r1");
    assert_eq!(assemble_cmd("goto PSR1"), "goto PSR1");
    assert_eq!(assemble_cmd("goto PSR127"), "goto PSR127");
    assert_eq!(
        MObjCmd::assemble("goto PSR128").unwrap_err(),
        MObjParseError::User {
            error: MObjParseErrorDetails::PsrOutOfRange
        }
    );
    assert_eq!(assemble_cmd("goto r4095"), "goto r4095");
    assert_eq!(
        MObjCmd::assemble("goto r4096").unwrap_err(),
        MObjParseError::User {
            error: MObjParseErrorDetails::GprOutOfRange
        }
    );
    assert_eq!(assemble_cmd("goto 0x10"), "goto 16");
    assert_eq!(assemble_cmd("goto r0x10"), "goto r16");
    assert_eq!(assemble_cmd("goto PSR0x10"), "goto PSR16");
}

fn test_cmd(s: &str) {
    assert_eq!(assemble_cmd(s), s);
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

    // TODO: Parse SetStream and SetButtonPage
    //test_cmd("set_stream r1, r2");
    test_cmd("set_nv_timer r1, r2");
    //test_cmd("set_button_page r1, r2");
    test_cmd("enable_button r1");
    test_cmd("disable_button r1");
    test_cmd("set_sec_stream r1, r2");
    test_cmd("popup_off");
    test_cmd("still_on");
    test_cmd("still_off");
    test_cmd("set_output_mode r1");
    //test_cmd("set_stream_ss r1, r2");
    test_cmd("bd_plus_msg r1, r2");
}
