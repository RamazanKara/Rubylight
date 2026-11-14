package com.limelight.binding.input;

import android.content.Context;
import android.content.SharedPreferences;
import android.view.InputDevice;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.UUID;

/**
 * Which gyroscope mapping each controller uses: Standard, the Flydigi preset or a named custom
 * profile with its own axes, inversions and sensitivity.
 *
 * A choice is stored under the controller's Android input descriptor, which tells apart two
 * pads of one model (Flydigi pads in Switch mode both report as "Pro Controller"), and under its
 * model key ({@link ControllerButtonMap#deviceKey}) for pads Nightjar drives over USB itself,
 * which have no input descriptor.
 *
 * Ported from jvanderv42's Nightjar fork (jvanderv42/nightjar, GPL-3.0, commit d0e14cf25,
 * app/src/main/java/com/limelight/binding/input/GyroMappingPreferences.java). The fork's
 * automatic choice matched two descriptors of its author's own pads; those include each pad's
 * unique ID, so the Flydigi preset is chosen by hand here.
 */
public final class GyroMappingPreferences {
    public static final String STANDARD = "standard";
    public static final String FLYDIGI = "flydigi";
    public static final String CUSTOM_PREFIX = "custom:";

    static final String STORE = "nightjar_gyro_mapping";
    private static final String DESCRIPTOR_PREFIX = "descriptor_";
    private static final String MODEL_PREFIX = "model_";
    private static final String PROFILES = "custom_profiles";
    // The fork author's measurement on a Flydigi Apex 4 and Vader 4 Pro in Switch mode.
    static final GyroAxisTransformer FLYDIGI_TRANSFORM = new GyroAxisTransformer(1, 2, 0, -1, 1, 1);

    private GyroMappingPreferences() {}

    /** One saved profile. Axes are a permutation of 0, 1, 2; signs are +1 or -1. */
    public static final class Profile {
        public final String id;
        public final String name;
        public final int[] axes;
        public final int[] signs;
        public final float horizontalSensitivity;
        public final float verticalSensitivity;

        public Profile(String id, String name, int[] axes, int[] signs) {
            this(id, name, axes, signs, 1f, 1f);
        }

        public Profile(String id, String name, int[] axes, int[] signs,
                       float horizontalSensitivity, float verticalSensitivity) {
            if (!GyroAxisTransformer.validSensitivity(horizontalSensitivity)
                    || !GyroAxisTransformer.validSensitivity(verticalSensitivity)) {
                throw new IllegalArgumentException("Sensitivity must be between 0.10 and 3.00");
            }
            if (name == null || name.trim().isEmpty() || axes == null || signs == null
                    || axes.length != 3 || signs.length != 3) {
                throw new IllegalArgumentException("Name and three axes/signs are required");
            }
            if (!validAxes(axes, signs)) {
                throw new IllegalArgumentException("Axes must be unique X, Y, Z with signs +1/-1");
            }
            this.id = id == null || id.isEmpty() ? UUID.randomUUID().toString() : id;
            this.name = name.trim();
            this.axes = axes.clone();
            this.signs = signs.clone();
            this.horizontalSensitivity = horizontalSensitivity;
            this.verticalSensitivity = verticalSensitivity;
        }

        public GyroAxisTransformer transformer() {
            return new GyroAxisTransformer(axes[0], axes[1], axes[2],
                    signs[0], signs[1], signs[2],
                    horizontalSensitivity, verticalSensitivity);
        }
    }

    /** Whether each source axis is used once and every sign is +1 or -1. */
    public static boolean validAxes(int[] axes, int[] signs) {
        if (axes == null || signs == null || axes.length != 3 || signs.length != 3) return false;
        boolean[] seen = new boolean[3];
        for (int i = 0; i < 3; i++) {
            if (axes[i] < 0 || axes[i] > 2 || seen[axes[i]] || (signs[i] != -1 && signs[i] != 1)) {
                return false;
            }
            seen[axes[i]] = true;
        }
        return true;
    }

    private static SharedPreferences preferences(Context context) {
        return context.getApplicationContext().getSharedPreferences(STORE, Context.MODE_PRIVATE);
    }

    public static List<Profile> profiles(Context context) {
        List<Profile> result = new ArrayList<>();
        try {
            JSONArray array = new JSONArray(preferences(context).getString(PROFILES, "[]"));
            for (int i = 0; i < array.length(); i++) {
                try {
                    JSONObject p = array.getJSONObject(i);
                    JSONArray axes = p.getJSONArray("axes");
                    JSONArray signs = p.getJSONArray("signs");
                    result.add(new Profile(p.getString("id"), p.getString("name"),
                            new int[] {axes.getInt(0), axes.getInt(1), axes.getInt(2)},
                            new int[] {signs.getInt(0), signs.getInt(1), signs.getInt(2)},
                            (float) p.optDouble("horizontalSensitivity", 1.0),
                            (float) p.optDouble("verticalSensitivity", 1.0)));
                } catch (JSONException | IllegalArgumentException ignored) {
                    // One damaged record must not drop the other profiles.
                }
            }
        } catch (JSONException ignored) {
            // Unreadable JSON reads as no profiles.
        }
        return result;
    }

    private static Profile profile(Context context, String id) {
        for (Profile p : profiles(context)) {
            if (p.id.equals(id)) return p;
        }
        return null;
    }

    private static void writeProfiles(SharedPreferences.Editor editor, List<Profile> profiles) {
        JSONArray array = new JSONArray();
        for (Profile p : profiles) {
            JSONObject json = new JSONObject();
            try {
                json.put("id", p.id);
                json.put("name", p.name);
                json.put("axes", new JSONArray(p.axes));
                json.put("signs", new JSONArray(p.signs));
                json.put("horizontalSensitivity", p.horizontalSensitivity);
                json.put("verticalSensitivity", p.verticalSensitivity);
                array.put(json);
            } catch (JSONException exception) {
                throw new IllegalStateException("Failed to serialize gyro profile", exception);
            }
        }
        editor.putString(PROFILES, array.toString());
    }

    public static void saveProfile(Context context, Profile profile) {
        List<Profile> list = profiles(context);
        boolean replaced = false;
        for (int i = 0; i < list.size(); i++) {
            if (list.get(i).id.equals(profile.id)) {
                list.set(i, profile);
                replaced = true;
                break;
            }
        }
        if (!replaced) list.add(profile);
        SharedPreferences.Editor editor = preferences(context).edit();
        writeProfiles(editor, list);
        editor.apply();
    }

    /** Deletes a profile; controllers that used it go back to Standard. */
    public static void deleteProfile(Context context, String id) {
        List<Profile> list = profiles(context);
        list.removeIf(profile -> profile.id.equals(id));
        SharedPreferences prefs = preferences(context);
        SharedPreferences.Editor editor = prefs.edit();
        writeProfiles(editor, list);
        String choice = CUSTOM_PREFIX + id;
        for (Map.Entry<String, ?> entry : prefs.getAll().entrySet()) {
            if (choice.equals(entry.getValue())) editor.remove(entry.getKey());
        }
        editor.apply();
    }

    private static String resolve(Context context, String stored) {
        if (FLYDIGI.equals(stored)) return FLYDIGI;
        if (stored != null && stored.startsWith(CUSTOM_PREFIX)
                && profile(context, stored.substring(CUSTOM_PREFIX.length())) != null) {
            return stored;
        }
        return STANDARD;
    }

    /** The choice for a controller's input descriptor: {@link #STANDARD}, {@link #FLYDIGI} or a custom one. */
    public static String getChoice(Context context, String descriptor) {
        if (descriptor == null || descriptor.isEmpty()) return STANDARD;
        return resolve(context, preferences(context).getString(DESCRIPTOR_PREFIX + descriptor, null));
    }

    /** Stores a choice under the descriptor and, for USB-driven pads of that model, the model key. */
    public static void setChoice(Context context, String descriptor, String modelKey, String choice) {
        String resolved = resolve(context, choice);
        SharedPreferences.Editor editor = preferences(context).edit();
        if (descriptor != null && !descriptor.isEmpty()) {
            if (STANDARD.equals(resolved)) editor.remove(DESCRIPTOR_PREFIX + descriptor);
            else editor.putString(DESCRIPTOR_PREFIX + descriptor, resolved);
        }
        if (modelKey != null && !modelKey.isEmpty()) {
            if (STANDARD.equals(resolved)) editor.remove(MODEL_PREFIX + modelKey);
            else editor.putString(MODEL_PREFIX + modelKey, resolved);
        }
        editor.apply();
    }

    public static void setChoice(Context context, InputDevice device, String choice) {
        setChoice(context, device.getDescriptor(), modelKey(device), choice);
    }

    public static String modelKey(InputDevice device) {
        return ControllerButtonMap.deviceKey(device.getVendorId(), device.getProductId(), device.getName());
    }

    private static GyroAxisTransformer transformerFor(Context context, String choice) {
        if (FLYDIGI.equals(choice)) return FLYDIGI_TRANSFORM;
        if (choice.startsWith(CUSTOM_PREFIX)) {
            Profile p = profile(context, choice.substring(CUSTOM_PREFIX.length()));
            if (p != null) return p.transformer();
        }
        return GyroAxisTransformer.identity();
    }

    /** The mapping for a controller Android reports as an input device. */
    public static GyroAxisTransformer transformer(Context context, String descriptor) {
        return transformerFor(context, getChoice(context, descriptor));
    }

    public static GyroAxisTransformer transformer(Context context, InputDevice device) {
        return transformer(context, device == null ? null : device.getDescriptor());
    }

    /** The mapping for a pad Nightjar drives over USB, by its model key. */
    public static GyroAxisTransformer transformerForModel(Context context, String modelKey) {
        if (modelKey == null || modelKey.isEmpty()) return GyroAxisTransformer.identity();
        return transformerFor(context, resolve(context,
                preferences(context).getString(MODEL_PREFIX + modelKey, null)));
    }
}
