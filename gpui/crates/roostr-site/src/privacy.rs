//! `/privacy` (`src/routes/privacy/+page.svelte`): what Roostr stores, where
//! it goes, and who can see it.

use crate::doc::{Block, Block::*, DocStyle};

pub(crate) fn style() -> DocStyle {
    DocStyle {
        h2_size: 20.,
        h2_margin: (32., 8.),
        updated_size: 14.,
        updated_margin: 28.,
        li_margin: 0.,
    }
}

pub(crate) static BLOCKS: &[Block] = &[
    Home,
    Title("Privacy policy"),
    Updated("Effective 13 September 2026"),
    P(
        "Roostr is a local-first notes app. It is built so that the people who run it — including us — cannot read your notes. This page describes exactly what is stored, where it travels, and what anyone along the way can see. It applies to the Roostr apps for iOS, the web app at roostr.space, and the default relay we operate.",
    ),
    H2(None, "No accounts, no analytics"),
    P(
        "Roostr has no user accounts. Your identity is a cryptographic key pair that is generated on your device (or imported by you). We never receive the private key. The apps contain no analytics, advertising or crash-reporting SDKs and do not send usage data to us or to anyone else.",
    ),
    H2(None, "What stays on your device"),
    Ul(&[
        "Your private key — in the iOS Keychain, or in your browser's storage for the web app.",
        "Your notes, tasks, tables, spaces and chats, and their full edit history.",
        "Keys for shared spaces you own or have been invited to.",
        "A list of the relays you sync with.",
    ]),
    P("Deleting the app, or choosing “Log out” in it, removes all of this from the device."),
    H2(None, "What leaves your device"),
    P(
        "To sync between your devices and with people you share spaces with, Roostr publishes your changes to Nostr relays as signed events. Before anything is sent it is encrypted end-to-end (NIP-44) with a key that only your devices — and, for shared spaces, the members you invited — hold. The identifiers attached to those events are blinded hashes, not your object names or ids.",
    ),
    P("A relay therefore sees:"),
    Ul(&[
        "your public key (the identity every event is signed with),",
        "encrypted ciphertext and its size,",
        "timestamps, and",
        "the IP address your device connects from, as any internet server does.",
    ]),
    P("A relay cannot see note contents, titles, structure, or who you share a space with."),
    H2(None, "The default relay"),
    P(
        "New installs sync through `wss://roostr-relay.fly.dev`, which we operate on Fly.io. It stores the encrypted events it receives so your other devices can fetch them, and it keeps ordinary connection logs (IP address, time, request size) for up to 30 days to run and protect the service. We do not sell or share this data, and we cannot decrypt the events. You can point Roostr at any other relay, or run your own with the open-source [RoostrRelay](https://github.com/Geep5/RoostrRelay), at any time; the app then never contacts ours.",
    ),
    H2(None, "Sharing"),
    P(
        "When you invite someone to a space, the space key is sent to them inside an encrypted gift wrap addressed to their public key; when you request to join, your public key is sent to the space owner the same way. Members of a shared space can read everything in that space, and the owner can rotate its key to exclude a member from future changes.",
    ),
    H2(None, "Public profile"),
    P(
        "If you choose to set a display name or avatar, it is published as a standard Nostr profile event so that other members can recognise you. It is not encrypted and is visible to anyone who queries a relay for your public key. Leave it empty if you prefer.",
    ),
    H2(None, "The website"),
    P(
        "roostr.space is a static site served from Fly.io. Fly's proxy records standard access logs (IP address, time, path, user agent). The site sets no tracking cookies and loads no third-party scripts.",
    ),
    H2(None, "Children"),
    P(
        "Roostr is not directed at children under 13 and does not knowingly collect information from them.",
    ),
    H2(None, "Your control"),
    P(
        "Because your data is encrypted with your key and stored under your identity, there is nothing we can hand over, correct or delete on your behalf beyond the encrypted events on our relay. To remove those, delete the objects in the app or ask us to purge everything published under your public key.",
    ),
    H2(None, "Changes and contact"),
    P(
        "If this policy changes we will update the date above and note the change on this page. Questions: [privacy@roostr.space](mailto:privacy@roostr.space). The source of every Roostr component is public at [github.com/Geep5](https://github.com/Geep5).",
    ),
];
