package com.limelight.console;

import com.limelight.nativeapi.NativeRadeon;

import org.json.JSONObject;

/** GET /radeon answers in each of the five states, as the host sends them. */
final class RadeonFixtures {
    private RadeonFixtures() {
    }

    static final String CHILL = "{\"id\":\"chill.enabled\",\"group\":\"chill\",\"kind\":\"toggle\","
            + "\"state\":\"configurable\",\"value\":false}";
    static final String CHILL_MAX = "{\"id\":\"chill.max_fps\",\"group\":\"chill\",\"kind\":\"integer\","
            + "\"state\":\"configurable\",\"value\":144,\"range\":{\"min\":30,\"max\":240,\"step\":10},\"unit\":\"fps\"}";
    static final String VSYNC = "{\"id\":\"vsync.mode\",\"group\":\"vsync\",\"kind\":\"choice\","
            + "\"state\":\"configurable\",\"value\":\"off\",\"options\":[{\"value\":\"off\",\"label\":\"Off\"},"
            + "{\"value\":\"always\",\"label\":\"Always on\"}]}";
    static final String FRAME_TARGET = "{\"id\":\"frame_rate_target.fps\",\"group\":\"frame_rate_target\","
            + "\"kind\":\"integer\",\"state\":\"configurable\",\"value\":120,\"range\":{\"min\":30,\"max\":240,\"step\":1},"
            + "\"unit\":\"fps\",\"restore\":{\"original\":120,\"written\":90}}";

    static String panel(String state, boolean hostControl, boolean writableNow, String extra, String... settings) {
        return "{\"revision\":3,\"state\":\"" + state + "\",\"host_control\":" + hostControl
                + ",\"writable_now\":" + writableNow + (extra == null ? "" : "," + extra)
                + ",\"gpus\":[{\"id\":9,\"name\":\"RX 7900 XT\",\"driver\":\"24.1\",\"settings\":["
                + String.join(",", settings) + "]}]}";
    }

    static String configurable() {
        return panel("configurable", true, true, null, CHILL, CHILL_MAX, VSYNC, FRAME_TARGET);
    }

    static NativeRadeon parse(String json) {
        try {
            return NativeRadeon.parse(new JSONObject(json));
        } catch (Exception e) {
            throw new IllegalStateException(e);
        }
    }

    static RadeonPanelModel model(String json) {
        return RadeonPanelModel.of(parse(json));
    }
}
