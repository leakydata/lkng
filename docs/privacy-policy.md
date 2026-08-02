# LKNG privacy policy

Last updated: 2026-08-01. Applies to the LKNG app and nothing else.

## The short version

**We do not collect your data, because there is no "we" that could.** LKNG
has no servers, no accounts, no analytics, no advertising SDKs and no
third-party trackers. Nobody operating this project receives your messages,
your location, your photos or a record that you use the app at all.

That is a structural claim, not a promise about our intentions: there is no
LKNG server for data to be sent to. It is also not the same as saying you
are anonymous. What LKNG genuinely does and does not protect is set out in
[anonymity-limitations.md](anonymity-limitations.md), which is reachable
from inside the app, and you should read it before deciding to trust this
with anything sensitive.

## What is stored on your device

- **Your identity key.** Sealed by the Android Keystore. It is the account —
  there is no password and no recovery email.
- **Your profile draft**, including your photo.
- **Your messages**, both received and the copies of ones you sent.
- **Favourites, private notes and blocks.** These never leave the device
  except inside a backup you create deliberately.
- **Your declared date of birth**, for the 18+ gate. Stored locally,
  published nowhere.
- **A hand-set location**, if you choose to set one.

Uninstalling the app deletes all of it. Without a recovery backup, that is
permanent — including your identity.

## What is published to the Freenet network

Publishing to Freenet means copying data to other people's computers. It is
public in the way a public noticeboard is public.

- **Your grid tile**, while you are visible: a headline, a small photo, a
  coarse age band, a position code, a rotating pseudonym, and a rotating
  public encryption key. It is published to a roughly 5 km cell.
- **Messages you send**, encrypted to the recipient. Nobody replicating them
  can read them or tell who sent them.

**Never published:** your exact location, any distance between you and
anyone else, your HIV status or any other health information, your date of
birth, your private notes, your favourites, or your block list.

Your position is converted to a coarse cell **on your device**, with a
stable random offset, before anything is shared. Exact coordinates never
leave your phone.

## The parts you cannot undo

Being honest about this matters more than the reassurance would be worth.

- **Published data cannot be recalled.** Once your tile has been copied to
  other peers, deleting the app does not delete those copies. Tiles expire
  from the network within about 12 hours because of how the contracts
  rotate, but nothing forces anyone to forget what they already saw.
- **A public photo grid is scrapeable.** Anyone can subscribe to a cell and
  collect the tiles in it. Rotating pseudonyms stop naive key-based
  tracking; they do not stop face matching. If your face is in the grid, it
  can be matched across time by anyone willing to do it. This is true of
  every app in this category, including the ones that do not say so.
- **Your IP address is visible to the peers you connect to.** Freenet's
  topology bounds this to a gateway and your neighbouring peers, not the
  whole network, but it is not anonymity and LKNG does not route over Tor.

## Age

LKNG is for adults, 18 and over. The app asks for a date of birth and
checks it. This is a self-declaration and we cannot verify it — real
verification would require a document check by an identity provider, which
is precisely the centralised party this project exists without. We would
rather state the limit than imply a protection that is not there.

## Children

We do not knowingly provide this app to anyone under 18. Because there are
no accounts and no server, there is no database from which a minor's data
could be deleted on request — the correct remedy is to uninstall the app,
which removes everything held locally.

## Reporting and safety

Reporting and blocking are built in. Blocking is immediate and local: a
blocked person's tiles and messages are dropped before anything is drawn.
Reports go to subscribable moderation feeds rather than to a company, and
you choose which feeds you trust.

## Changes

Changes to this policy will appear in this file, whose history is public in
the repository. There is no mailing list to notify, because there is no list
of users.

## Contact

The project lives at <https://github.com/leakydata/lkng>. Issues and
security reports go there. Nobody at this project can look up an account,
recover a key, or remove something already published — not as a matter of
policy, but because the capability does not exist.
