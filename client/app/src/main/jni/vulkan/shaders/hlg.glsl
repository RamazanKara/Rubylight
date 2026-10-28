// BT.2100 HLG, scalar so app/src/test/native/vulkan_output_format_test.cpp compiles it as C++ too.

// The system gamma of the OOTF on the BT.2100 reference display (1000 nits peak)
const float HLG_SYSTEM_GAMMA = 1.2;

// Inverse OETF: the non-linear HLG signal to normalised scene light, both 0 to 1
float hlgInverseOetf(float e) {
    const float a = 0.17883277;
    const float b = 0.28466892;
    const float c = 0.55991073;
    e = max(e, 0.0f);
    return e <= 0.5 ? e * e / 3.0 : (exp((e - c) / a) + b) / 12.0;
}
