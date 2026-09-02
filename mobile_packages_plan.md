mobile first, desktop where it make sense

Tier 1

1 permissions — unified request/check/status API

2 notifications — local scheduling + push token registration + tap-handling/deep-link into the app

3 applinks — deep links + universal links/app links: URL registration

Tier 2

1 securestore — Keychain (iOS) / Keystore-backed EncryptedSharedPreferences or equivalent (Android)

2 camera — capture photo/video + preview surface as a Facet-embeddable native view (your Apple Maps escape-hatch test proves the pattern). iOS: AVFoundation; Android: Camera2 — SUPERSEDED, see plans/camera.md §1. The original line here said CameraX; measurement put it at 845× the dex for the quirk database, and the trade was taken the other way

3 location — one-shot + continuous, accuracy tiers, background-mode flags

Tier 3

1 sensors — accelerometer, gyro, magnetometer, barometer

2 haptics

3 share

4 biometrics

5 filepicker

Tier 4

1 Bluetooth/BLE

2 Contacts/calendar

3 In-app purchases

