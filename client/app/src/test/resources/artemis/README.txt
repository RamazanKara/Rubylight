Artemis import fixtures

Artemis (https://github.com/ClassicOldSong/moonlight-android, branch moonlight-noir) is GPL-3.0, like
Nightjar. These files are test data for TouchLayoutImport.

wiki-custom-special-keys.json
  Real sample, copied verbatim (tabs kept) from the Artemis wiki page "Custom-Special-Keys"
  (https://github.com/ClassicOldSong/moonlight-android/wiki/Custom-Special-Keys, wiki commit
  c83be908ed8a50310612015ace2fc107797415fb). This is the format "Import Custom Keys" reads
  (utils/KeyConfigHelper.ShortcutFile).

issue-521-mouse-keys.json
  Real entries, copied from Artemis issue #521 ("allow for duplicate virtual keys",
  https://github.com/ClassicOldSong/moonlight-android/issues/521). The issue shows the three entries
  only; they are wrapped here in the documented {"data": [...]} envelope from the wiki page above.

derived-keyboard-layout-2400x1080.json
  NOT a real export. No exported keyboard layout was found in the Artemis repository, wiki, issues or
  discussions. Built by following the exporter at source commit
  c5cf27f4dc822db0e863c4691e7a70c74bea977a exactly:
  - preferences/StreamSettings.java getJsonContent(): new Gson().toJson(prefs.getAll()) of the
    "OSC_Keyboard" preferences (compact JSON object, element ID to string);
  - keyboard/KeyBoardControllerConfigurationLoader.saveProfile(): each value is
    element.getConfiguration().toString();
  - keyboard/keyBoardVirtualControllerElement.getConfiguration(): LEFT, TOP, WIDTH, HEIGHT (pixels,
    layout margins and size), ENABLED, HIDDEN, in that order;
  - element IDs from assets/config/keyboard.json and createDefaultLayout(): rocker_1, dpad_1,
    key_<Android key code>, m_<mouse code> (m_s_3 is the toggling right button, m_9..m_11 touchpad
    areas), custom_<Custom Keys id>.
  Positions are for a 2400x1080 landscape screen with Artemis's key size of 120 px.
