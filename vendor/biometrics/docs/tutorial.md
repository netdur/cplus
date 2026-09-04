# Tutorial

## 1. Depend on it

```toml
[dependencies]
biometrics = "*"
```

## 2. Ask

```cplus
import "biometrics/biometrics" as bio;

fn answered(o: bio::Outcome, ctx: *u8) {
    match o {
        bio::Outcome::Ok => { unlock(); }
        bio::Outcome::Cancelled => { }              // they said no. do nothing.
        bio::Outcome::NotEnrolled => { show("Set up Face ID in Settings"); }
        bio::Outcome::LockedOut => { show("Too many attempts — use your passcode"); }
        _ => { show("Could not verify"); }
    }
    return;
}

let _o: bio::Outcome = bio::authenticate("Unlock your notes", answered);
```

The `reason` is **shown to the person**. Write it as a sentence completing
"…to", because that is how iOS renders it.

## 3. Hide the button when there is nothing to ask with

```cplus
if bio::available() { show_unlock_button(); }
```

## 4. Name the sensor, carefully

```cplus
let label: str = match bio::kind() {
    bio::Kind::Face => "Face ID",
    bio::Kind::Fingerprint => "Touch ID",
    _ => "Biometrics",
};
```

On Android this is a **guess** — the framework will not say which sensor a
device has. See the [guide](guide.md#android-will-not-say-which).

## 5. Do not use it as a login

```cplus
// WRONG: this proves whose DEVICE it is, not who the user is.
if passed { log_in_as(current_user); }

// Right: unlock a token you already have.
if passed { let _g = securestore::get("refresh_token", into: t); }
```
