**Tier 1 — almost every app needs these**
- Networking: HTTP client, WebSocket, TLS pinning hooks
- Persistent storage: key-value prefs, secure storage (Keychain/Keystore), SQLite
- File system: app dirs, temp/cache, document picker, file sharing
- Permissions: unified request/check API (this is the plumbing every other plugin depends on)
- Camera and image/video picker
- Geolocation (foreground + background) and geocoding
- Push notifications (APNs/FCM) and local notifications
- Deep links / universal links / app links
- Clipboard, share sheet, open-URL/mail/phone intents
- Device info, app info (version, build), battery, connectivity/network status
- Haptics and vibration
- Keyboard handling (safe area, insets, IME) — often overlooked, always painful
- Splash screen and app icon generation
- WebView

**Tier 2 — common, expected in a mature ecosystem**
- Biometrics / local auth (Face ID, fingerprint)
- Sign in with Apple / Google auth
- In-app purchases and subscriptions (StoreKit 2 + Google Play Billing)
- Maps (Apple/Google/Mapbox abstraction)
- Audio playback/recording, video player
- Image caching and manipulation (resize, crop, compress)
- Background tasks / work scheduling
- App lifecycle and app-state events
- Sensors: accelerometer, gyroscope, pedometer, barometer
- Contacts, calendar
- Bluetooth LE and NFC
- Speech-to-text, text-to-speech
- Localization/intl (dates, numbers, plurals, RTL)
- Accessibility bridge (screen reader labels, dynamic type)
- Analytics/crash reporting hooks (Crashlytics, Sentry adapters)
- Firebase suite adapters
- Ads (AdMob) — controversial but heavily requested

**Tier 3 — differentiators / long tail**
- Health (HealthKit / Health Connect)
- Widgets, live activities, App Clips / Instant Apps
- Wear OS / watchOS companions
- OTA code updates (like Expo Updates / CodePush) — big adoption driver
- Screen capture prevention, secure flag
- Printing, PDF rendering
- AR (ARKit/ARCore)
- Payments sheets (Apple Pay / Google Pay)
- App review prompt, store links
- QR/barcode scanning
- ML Kit bindings (OCR, face detection)
