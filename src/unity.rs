//! Typed views of the Unity objects this crate reads. Field names are the ones in the type trees
//! stored in the bundles, which is how rabex matches them; fields a decoder does not use are
//! left out and skipped while reading.

#![allow(non_snake_case)]

use serde::Deserialize;

use crate::bundle::StreamingInfo;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct PPtr {
    pub m_FileID: i32,
    pub m_PathID: i64,
}

impl PPtr {
    /// Whether it points at an object of the same file (a null pointer does not).
    pub fn local(&self) -> Option<i64> {
        (self.m_FileID == 0 && self.m_PathID != 0).then_some(self.m_PathID)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
pub struct ColorRGBA {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

#[derive(Debug, Deserialize)]
pub struct ComponentPair {
    pub component: PPtr,
}

#[derive(Debug, Deserialize)]
pub struct GameObject {
    pub m_Component: Vec<ComponentPair>,
    pub m_Name: String,
    #[serde(default)]
    pub m_IsActive: bool,
}

#[derive(Debug, Deserialize)]
pub struct Transform {
    pub m_GameObject: PPtr,
    pub m_LocalRotation: Quat,
    pub m_LocalPosition: Vec3,
    pub m_LocalScale: Vec3,
    pub m_Children: Vec<PPtr>,
    pub m_Father: PPtr,
}

#[derive(Debug, Deserialize)]
pub struct MeshFilter {
    pub m_GameObject: PPtr,
    pub m_Mesh: PPtr,
}

/// MeshRenderer and SkinnedMeshRenderer share these fields; the skinned one adds the rest.
#[derive(Debug, Deserialize)]
pub struct Renderer {
    pub m_GameObject: PPtr,
    #[serde(default)]
    pub m_Enabled: bool,
    pub m_Materials: Vec<PPtr>,
    #[serde(default)]
    pub m_Mesh: PPtr,
    #[serde(default)]
    pub m_Bones: Vec<PPtr>,
    #[serde(default)]
    pub m_RootBone: PPtr,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SubMesh {
    pub firstByte: u32,
    pub indexCount: u32,
    pub topology: i32,
    pub baseVertex: u32,
    pub firstVertex: u32,
    pub vertexCount: u32,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
pub struct ChannelInfo {
    pub stream: u8,
    pub offset: u8,
    pub format: u8,
    pub dimension: u8,
}

#[derive(Debug, Default, Deserialize)]
pub struct VertexData {
    pub m_VertexCount: u32,
    pub m_Channels: Vec<ChannelInfo>,
    pub m_DataSize: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
pub struct Matrix4x4 {
    pub e00: f32,
    pub e01: f32,
    pub e02: f32,
    pub e03: f32,
    pub e10: f32,
    pub e11: f32,
    pub e12: f32,
    pub e13: f32,
    pub e20: f32,
    pub e21: f32,
    pub e22: f32,
    pub e23: f32,
    pub e30: f32,
    pub e31: f32,
    pub e32: f32,
    pub e33: f32,
}

impl Matrix4x4 {
    /// Row-major, `m[row][col]`, as Unity's `eRC` names say.
    pub fn rows(&self) -> [[f32; 4]; 4] {
        [
            [self.e00, self.e01, self.e02, self.e03],
            [self.e10, self.e11, self.e12, self.e13],
            [self.e20, self.e21, self.e22, self.e23],
            [self.e30, self.e31, self.e32, self.e33],
        ]
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct PackedBitVector {
    #[serde(default)]
    pub m_NumItems: u32,
    #[serde(default)]
    pub m_Range: f32,
    #[serde(default)]
    pub m_Start: f32,
    #[serde(default)]
    pub m_Data: Vec<u8>,
    #[serde(default)]
    pub m_BitSize: u8,
}

#[derive(Debug, Default, Deserialize)]
pub struct CompressedMesh {
    pub m_Vertices: PackedBitVector,
    pub m_UV: PackedBitVector,
    pub m_Normals: PackedBitVector,
    pub m_Tangents: PackedBitVector,
    pub m_Weights: PackedBitVector,
    pub m_NormalSigns: PackedBitVector,
    pub m_TangentSigns: PackedBitVector,
    pub m_FloatColors: PackedBitVector,
    pub m_BoneIndices: PackedBitVector,
    pub m_Triangles: PackedBitVector,
    pub m_UVInfo: u32,
}

#[derive(Debug, Deserialize)]
pub struct Mesh {
    pub m_Name: String,
    pub m_SubMeshes: Vec<SubMesh>,
    pub m_BindPose: Vec<Matrix4x4>,
    pub m_BoneNameHashes: Vec<u32>,
    pub m_RootBoneNameHash: u32,
    pub m_MeshCompression: u8,
    pub m_IndexFormat: i32,
    pub m_IndexBuffer: Vec<u8>,
    pub m_VertexData: VertexData,
    pub m_CompressedMesh: CompressedMesh,
    #[serde(default)]
    pub m_StreamData: StreamingInfo,
}

#[derive(Debug, Deserialize)]
pub struct GLTextureSettings {
    pub m_FilterMode: i32,
    pub m_WrapU: i32,
    pub m_WrapV: i32,
}

#[derive(Debug, Deserialize)]
pub struct Texture2D {
    pub m_Name: String,
    pub m_Width: i32,
    pub m_Height: i32,
    pub m_CompleteImageSize: i32,
    pub m_TextureFormat: i32,
    pub m_MipCount: i32,
    #[serde(default)]
    pub m_ImageCount: i32,
    #[serde(default)]
    pub m_TextureDimension: i32,
    pub m_TextureSettings: GLTextureSettings,
    #[serde(default)]
    pub m_ColorSpace: i32,
    #[serde(rename = "image data")]
    pub image_data: Vec<u8>,
    #[serde(default)]
    pub m_StreamData: StreamingInfo,
}

#[derive(Debug, Deserialize)]
pub struct TexEnv {
    pub m_Texture: PPtr,
    pub m_Scale: Vec2,
    pub m_Offset: Vec2,
}

#[derive(Debug, Deserialize)]
pub struct PropertySheet {
    pub m_TexEnvs: Vec<(String, TexEnv)>,
    pub m_Floats: Vec<(String, f32)>,
    pub m_Colors: Vec<(String, ColorRGBA)>,
}

#[derive(Debug, Deserialize)]
pub struct Material {
    pub m_Name: String,
    pub m_Shader: PPtr,
    pub m_ShaderKeywords: String,
    #[serde(default)]
    pub m_CustomRenderQueue: i32,
    pub m_SavedProperties: PropertySheet,
}

#[derive(Debug, Default, Deserialize)]
pub struct StreamedClip {
    pub data: Vec<u32>,
    pub curveCount: u32,
}

#[derive(Debug, Default, Deserialize)]
pub struct DenseClip {
    pub m_FrameCount: i32,
    pub m_CurveCount: u32,
    pub m_SampleRate: f32,
    pub m_BeginTime: f32,
    pub m_SampleArray: Vec<f32>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ConstantClip {
    pub data: Vec<f32>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Clip {
    pub m_StreamedClip: StreamedClip,
    pub m_DenseClip: DenseClip,
    pub m_ConstantClip: ConstantClip,
}

#[derive(Debug, Default, Deserialize)]
pub struct OffsetPtrClip {
    pub data: Clip,
}

#[derive(Debug, Default, Deserialize)]
pub struct ClipMuscleConstant {
    pub m_Clip: OffsetPtrClip,
    pub m_StartTime: f32,
    pub m_StopTime: f32,
    pub m_LoopTime: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
pub struct GenericBinding {
    pub path: u32,
    pub attribute: u32,
    pub typeID: i32,
    pub customType: u8,
    pub isPPtrCurve: u8,
}

#[derive(Debug, Default, Deserialize)]
pub struct ClipBindingConstant {
    pub genericBindings: Vec<GenericBinding>,
}

#[derive(Debug, Deserialize)]
pub struct AnimationClip {
    pub m_Name: String,
    pub m_Legacy: bool,
    pub m_Compressed: bool,
    pub m_RotationCurves: Vec<serde::de::IgnoredAny>,
    pub m_CompressedRotationCurves: Vec<serde::de::IgnoredAny>,
    pub m_EulerCurves: Vec<serde::de::IgnoredAny>,
    pub m_PositionCurves: Vec<serde::de::IgnoredAny>,
    pub m_ScaleCurves: Vec<serde::de::IgnoredAny>,
    pub m_FloatCurves: Vec<serde::de::IgnoredAny>,
    pub m_SampleRate: f32,
    pub m_WrapMode: i32,
    pub m_MuscleClip: ClipMuscleConstant,
    pub m_ClipBindingConstant: ClipBindingConstant,
}

#[derive(Debug, Deserialize)]
pub struct SerializedShader {
    pub m_Name: String,
}

#[derive(Debug, Deserialize)]
pub struct Shader {
    pub m_Name: String,
    pub m_ParsedForm: SerializedShader,
}

#[derive(Debug, Deserialize)]
pub struct Animator {
    pub m_GameObject: PPtr,
    pub m_Avatar: PPtr,
    pub m_Controller: PPtr,
}

#[derive(Debug, Deserialize)]
pub struct Avatar {
    pub m_Name: String,
    pub m_TOS: Vec<(u32, String)>,
}

#[derive(Debug, Deserialize)]
pub struct AnimatorController {
    pub m_Name: String,
    pub m_AnimationClips: Vec<PPtr>,
}

#[derive(Debug, Deserialize)]
pub struct AssetInfo {
    pub asset: PPtr,
}

#[derive(Debug, Deserialize)]
pub struct AssetBundle {
    pub m_Name: String,
    pub m_Container: Vec<(String, AssetInfo)>,
    pub m_Dependencies: Vec<String>,
}
