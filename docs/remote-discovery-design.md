# Design: AirDrop-style pairing (discovery + confirm on the Mac)

Status: built. 4a (discovery) and 4b (confirm on the Mac) are specified in
`docs/remote-protocol.md`, "Discovery" and "Confirmed pairing".

## Goal

Pairing a phone should look like AirDrop: the phone lists the Macs near it by
name, you tap one, and you confirm on the Mac. No address, no port, no code to
type. Reconnecting should keep working when the Mac's IP address changes.

## What is wrong today

- **Typed pairing needs an address.** `192.168.1.24:47285` is the one thing on
  the Pair screen a person should never have to know. The QR code avoids it,
  but only if there is a camera to hand and the Mac's screen is in view.
- **The saved address goes stale.** The phone keeps the `addr` from the moment
  it paired (`HostTarget.host/port`). When DHCP hands the Mac a new address, or
  the port changes in Settings › Advanced, the LAN path stops working. The
  phone then falls back to the relay if there is one, and fails if there isn't,
  until the user pairs again.
- **Typed pairing trusts whoever answers.** With no QR there is no host key to
  check, so the phone pins the first key it meets. The threat model accepts
  this as the cost of manual entry ("Threat model (v2)").

## Proposal

### 1. The Mac announces itself (Bonjour)

While remote control is on, the host advertises `_fletch._tcp` on its listen
port. The TXT record holds:

- `name`: the machine name, as `host_info()` reports it.
- `id`: the host ID (its public key, base64url). It is already public and is
  what the relay routes on.

The record is a hint, not a credential. Anyone on the LAN can announce any
name and id, so nothing trusts it. The handshake and the confirmation code are
what authenticate.

### 2. The phone uses discovery for every connection, not just pairing

- **Pair screen:** a "Macs nearby" list built from the browse results, showing
  names only, AirDrop style.
- **Reconnect:** `candidatesFor` puts the address of the discovered instance
  whose `id` matches the pinned host key first, then the saved address, then
  the relay. A Mac whose IP or port changed is found again without re-pairing.
  The saved `addr` remains only as a fallback for networks that block
  multicast (guest Wi-Fi, client isolation).

### 3. Pairing is confirmed on the Mac, with a code both screens show

1. On the Mac: Settings › Remote control › **Pair a device** opens a pairing
   window. This already exists as the 5-minute invite, and it already picks
   the access preset. Outside a window the host refuses pair
   requests outright, so nobody on the LAN can spam prompts. This matches
   AirDrop's "Everyone for 10 minutes".
2. On the phone, the user taps the Mac. The phone runs the usual Noise XX
   handshake and sends a new first frame, `pair_request { device, commit }`.
3. Both ends derive a **6-digit code from the Noise handshake hash** and two
   nonces exchanged commit-then-reveal. The hash alone would not do: a party in
   the middle chooses its own ephemeral keys and could grind a million of them
   offline until both sides showed the same digits. The device commits to its
   nonce before seeing the host's, so neither side can choose after the other. The host's nonce still reaches a party in the middle before
   any prompt shows, letting it hang up unseen and roll again, so each pairing
   window answers at most five requests: five in a million per window. The phone shows "Confirm on your Mac: 482 913". The
   Mac shows "Alex's iPhone wants to connect · 482 913 · Accept / Decline".
4. Accept registers the device key exactly as `pair` does today and answers
   with the same result: `deviceId`, `host`, `relay`, `protocol`. Decline, or
   60 s without an answer, closes the connection with `4003`.

This is the numeric comparison Bluetooth uses. An attacker in the middle
produces two different handshakes and therefore two different codes, so the
"trusts whoever answers" gap of typed pairing closes. It also requires someone
at the Mac, which today's code does too.

### What stays

- **QR pairing** stays unchanged. iOS's Camera app opens the `fletch://pair`
  link, and it is the only way to pair from another network: discovery is
  LAN-only by nature.
- **Typed code + address** moves behind "Can't find your Mac?" on the phone,
  for networks where multicast is blocked.
- **The relay** gains no new role. Pairing over it still needs the QR, because
  a code typed against the relay would make the relay operator the party the
  phone trusts first.

## Compatibility

- Hosts that predate this do not advertise, so they never appear in the nearby
  list and are never sent `pair_request`. No version check is needed.
- `pair` and `hello` are unchanged. `pair_request` is a third allowed first
  frame. A host that sees it outside a pairing window closes with `4003`.

## Platform work and risks

- **iOS browsing:** use `NWBrowser` in a small Swift plugin, following the
  `plugins/push` pattern. It needs `NSBonjourServices = [_fletch._tcp]` and an
  `NSLocalNetworkUsageDescription` in `Info.ios.plist`, which today has
  neither. The phone already dials LAN addresses, so the local-network prompt
  is not new. `NWBrowser` does not need the multicast entitlement. A
  Rust-side mDNS client would, which is why browsing should not live in Rust.
- **Host advertising:** the pure-Rust `mdns-sd` crate, in `fletch-core`'s
  `remote/discovery.rs`, so the Mac app and the headless Linux host
  (`fletch-host`) announce through the same code. It shares port 5353 with
  mDNSResponder or Avahi rather than talking to either, and publishes its own
  host name, `fletch-<label>.local`, so it never contends for the machine's.
- **Desktop UI:** an incoming-request prompt (sheet or notification) on the
  Mac, available while Settings is closed but only during a pairing window.
- **Desktop as client:** Paired hosts › Add a host can use the same browse
  later. Out of scope for the first cut.

## Suggested slicing

- **4a: discovery (low risk, fixes the stale-address bug on its own).** Mac
  advertising, a phone browse plugin, discovered address first in
  `candidatesFor`, and a "Macs nearby" list on the Pair screen that fills in
  the address (still with a typed code). No protocol change.
- **4b: confirm on the Mac.** The `pair_request` op, the handshake-hash code,
  and the Mac prompt. Spec first, then host, then phone. The typed code moves
  behind "Can't find your Mac?".

## Decisions

1. Linux hosts announce too, from 4a on.
2. The Mac's accept prompt is a floating sheet shown during the pairing
   window, not a system notification.
3. (4b) No `pair=1` TXT flag: the host answers a request outside a pairing
   window with an error the phone shows, which needs no re-announcing as
   windows open and close.
4. (4a) The host's `.local` name is derived from its key, so a paired device
   reconnects by name without browsing; the name races the saved address
   inside the existing LAN budget. Browsing is only for the Pair screen's
   list, and its TXT record carries the port so no SRV resolution is needed.
