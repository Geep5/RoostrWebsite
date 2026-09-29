<script lang="ts">
	/** Human/private chat stays local; exchanges send immutable mailbox envelopes. */
	import { guestAgents, type AgentEndpoint, type AgentMessage, type ObjectJSON } from "$lib/types";
	import { chatMessages, isLegacyExchange, objectChatMessages, replyRecipients, uniqueEndpoints } from "$lib/chat";
	import { endpointName, loadObjectAgents } from "$lib/conversations";
	import type { ObjectAgentOption } from "$lib/threads";
	import { goto } from "$app/navigation";
	import { chat, mailbox, settings } from "$lib/api";
	import { store } from "$lib/data.svelte";
	import { objectIcon } from "$lib/icons";
	import { loadDraft, saveDraft } from "$lib/drafts";
	import EmojiPicker from "./EmojiPicker.svelte";
	import { renderMarkdown } from "$lib/markdown";
	import { onMount, untrack } from "svelte";
	import { harnessFetch, pairedSession, onPairingChange } from "$lib/local-transport";

	let {
		object,
		full = false,
		pagemode = false,
		threadId = "__discussion__",
		onchanged,
		onexchange,
	}: {
		object: ObjectJSON;
		full?: boolean;
		pagemode?: boolean;
		/** Which conversation in this object; the human thread by default. */
		threadId?: string;
		onchanged: () => Promise<void>;
		onexchange?: (threadId: string) => void;
	} = $props();

	const messages = $derived(threadId === "__discussion__" ? objectChatMessages(object) : chatMessages(object, threadId));
	const conversation = $derived(object.conversations?.find((c) => c.id === threadId));
	const legacy = $derived(isLegacyExchange(object, threadId));
	// The object's one chat (__discussion__) is a discussion even though it
	// shows envelopes: tagging sends one, an untagged line stays a chat line.
	// Only an opened exchange thread is reply-to-all.
	const isExchange = $derived(threadId !== "__discussion__" && (legacy || conversation?.kind === "a2a" || messages.some((m) => m.mailbox)));
	const readOnly = $derived(legacy || (isExchange && !!conversation?.closed)
		|| (threadId !== "__discussion__" && !conversation && !messages.length));
	const members = $derived(uniqueEndpoints(messages.flatMap((m) => m.mailbox ? [m.mailbox.message.sender, ...m.mailbox.message.recipients] : [])));
	const exchangeTitle = $derived(conversation?.title || messages.find((m) => m.mailbox)?.mailbox?.message.title || "Exchange");

	const messageById = $derived(new Map(messages.map((m) => [m.id, m])));

	let open = $state(false);
	let composerEl = $state<HTMLTextAreaElement>();
	const isOpen = $derived(full || pagemode || open);

	/** Anytype's phone idiom: the discussion opens as its own page. */
	function openDiscussion() {
		if (matchMedia("(max-width: 720px)").matches) {
			const prefix = location.pathname.startsWith("/app") ? "/app" : "";
			void goto(`${prefix}/chat/${object.id}`);
			return;
		}
		open = true;
	}

	// Land on the newest message, the way any chat does. This used to apply
	// only to the phone's full-page mode, so the drawer opened at the top of
	// the thread and a long history hid the very message you came to read.
	let messagesEl = $state<HTMLDivElement>();
	/** Within a message or so of the end - the reader is following along. */
	function atBottom(el: HTMLDivElement): boolean {
		return el.scrollHeight - el.scrollTop - el.clientHeight < 120;
	}
	function toBottom(el: HTMLDivElement) {
		el.scrollTop = el.scrollHeight;
	}
	/**
	 * Land at the end once per opened thread.
	 *
	 * Tracked by id rather than by depending on the prop: `object` is replaced
	 * wholesale every time the parent refetches after a commit, so an effect
	 * that merely reads it re-ran on each arriving message and dragged the
	 * reader back down mid-history. Cleared on close so reopening lands again.
	 */
	let landedOn = "";
	$effect(() => {
		const id = `${object.id}:${threadId}`;
		if (!isOpen) {
			landedOn = "";
			return;
		}
		if (landedOn === id) return;
		const el = messagesEl;
		if (!el) return; // container not bound yet; this re-runs when it is
		landedOn = id;
		// After paint, or scrollHeight is still the previous thread's.
		requestAnimationFrame(() => toBottom(el));
	});
	// On new messages: pagemode always follows, as it did. The drawer follows
	// only when the reader is already at the end, so scrolling back through
	// history is not yanked away by an arriving reply.
	$effect(() => {
		void messages.length;
		const el = messagesEl;
		if (!el) return;
		if (pagemode || atBottom(el)) requestAnimationFrame(() => toBottom(el));
	});
	$effect(() => {
		if (isOpen) composerEl?.focus();
	});
	// A half-typed message survives the pane switching to Properties (which
	// unmounts the chat) and moving between objects: kept per object + thread.
	// The first object/thread only seeds these; the effect below follows changes.
	let draftFor = untrack(() => `${object.id}|${threadId}`);
	let draft = $state(untrack(() => loadDraft(object.id, threadId)));
	$effect(() => {
		const key = `${object.id}|${threadId}`;
		if (key === draftFor) return;
		draftFor = key;
		draft = loadDraft(object.id, threadId);
	});
	$effect(() => {
		const [id, thread] = draftFor.split("|");
		saveDraft(id, thread, draft);
	});
	let replyTo = $state("");
	let pickerFor = $state("");
	let sending = $state(false);
	let sendError = $state("");
	let me = $state("");
	let identityError = $state("");
	let privateRecipient = $state("");
	let requestReply = $state(true);
	let retryError = $state("");
	let retrying = $state<string[]>([]);
	let pendingSend: AgentMessage | undefined;

	// ── @-mentions: Discord-style agent tag menu above the composer ──
	// A channel's own discussion wakes no agent on its own (the harness
	// routes those events to serving convergence, not a turn): the only way
	// to reach an agent from chat is an addressed mailbox envelope, and the
	// @ menu is how you address one. The roster is the same source as the
	// exchange picker's: the space's agents plus objects that name one.
	let tagOptions = $state<ObjectAgentOption[] | null>(null);
	let mention = $state<{ start: number; query: string } | null>(null);
	let mentionIndex = $state(0);
	/** Agents tagged for the next send; a deleted @Name untags at send time. */
	let tagged = $state<ObjectAgentOption[]>([]);

	async function ensureTagOptions(): Promise<void> {
		if (tagOptions) return;
		try {
			tagOptions = await loadObjectAgents(object);
		} catch {
			tagOptions = [];
		}
	}

	const mentionMatches = $derived.by(() => {
		if (!mention || !tagOptions) return [];
		const q = mention.query.toLowerCase();
		return tagOptions
			.filter((option) => !tagged.some((t) => t.endpoint.objectId === option.endpoint.objectId))
			.filter((option) => !q || option.agentName.toLowerCase().includes(q) || option.name.toLowerCase().includes(q))
			.slice(0, 8);
	});

	/** Open/refresh/close the menu from the caret: a `@…` token ends at the caret. */
	function updateMention() {
		const el = composerEl;
		if (!el) return;
		// A tag deleted from the text untags immediately, so the menu offers
		// that agent again (the seeded @Name must not pin it out of the list).
		tagged = tagged.filter((option) => draft.includes(`@${option.agentName}`));
		const caret = el.selectionStart ?? 0;
		const hit = /(?:^|\s)@([^@\n]*)$/.exec(draft.slice(0, caret));
		if (!hit) {
			mention = null;
			return;
		}
		const query = hit[1];
		// Text has no chips: a completed tag is only `@Name ` in the draft.
		// Once the caret moves past it the token still matches, so close
		// explicitly or the menu trails the rest of the message.
		if (tagged.some((option) => query.startsWith(`${option.agentName} `))) {
			mention = null;
			return;
		}
		if (!mention || mention.query !== query) mentionIndex = 0;
		mention = { start: caret - query.length - 1, query };
		void ensureTagOptions();
	}

	function pickMention(option: ObjectAgentOption) {
		const el = composerEl;
		const at = mention;
		if (!el || !at) return;
		const caret = el.selectionStart ?? at.start;
		const label = `@${option.agentName}`;
		// One trailing space so the next word doesn't glue onto the tag.
		const after = draft.slice(caret).startsWith(" ") ? draft.slice(caret) : ` ${draft.slice(caret)}`;
		draft = `${draft.slice(0, at.start)}${label}${after}`;
		if (!tagged.some((t) => t.endpoint.objectId === option.endpoint.objectId)) tagged = [...tagged, option];
		mention = null;
		requestAnimationFrame(() => {
			el.focus();
			const pos = at.start + label.length + 1;
			el.setSelectionRange(pos, pos);
			el.style.height = "auto";
			el.style.height = `${el.scrollHeight}px`;
		});
	}

	/** An object with an agent opens its composer already addressed: `@Name `
	 *  per guest agent, tagged, caret after - you just type. Only ever fills
	 *  an empty draft, so a message in progress is never touched. */
	async function seedMentions(): Promise<void> {
		if (draft !== "" || readOnly) return;
		await ensureTagOptions();
		if (draft !== "" || !tagOptions?.length) return;
		tagged = tagOptions;
		draft = tagOptions.map((option) => `@${option.agentName} `).join("");
		requestAnimationFrame(() => {
			const el = composerEl;
			if (!el) return;
			el.setSelectionRange(draft.length, draft.length);
			el.style.height = "auto";
			el.style.height = `${el.scrollHeight}px`;
		});
	}

	/** Tags whose @Name survived editing and are present in this text. */
	function liveTags(text: string): ObjectAgentOption[] {
		return tagged.filter((option) => text.includes(`@${option.agentName}`));
	}
	const replyMessage = $derived((replyTo ? messageById.get(replyTo) : messages.findLast((m) => m.mailbox))?.mailbox?.message);
	const replyAll = $derived(replyMessage ? replyRecipients(replyMessage, { objectId: object.id, agentId: "" }) : []);
	const audience = $derived(privateRecipient ? replyAll.filter((endpoint) => endpoint.objectId === privateRecipient) : replyAll);

	async function loadIdentity() {
		try {
			const result = await settings.fetch();
			me = result.authorId;
			identityError = "";
		} catch (error) {
			me = "";
			identityError = error instanceof Error ? error.message : "Discussion identity is unavailable while offline.";
		}
	}

	$effect(() => {
		void paired;
		void loadIdentity();
	});

	// ── Agent presence ─────────────────────────────────────────────
	// The paired harness reports live turn state on its authenticated surface;
	// while the discussion is open we poll it so the user sees the
	// agent composing (typing dots) or failing (warning row).
	let paired = $state(pairedSession() !== null);
	let presenceError = $state("");
	function refreshPairing() {
		paired = !!pairedSession();
	}
	onMount(() => {
		refreshPairing();
		void seedMentions();
		return onPairingChange(refreshPairing);
	});
	interface AgentPresence {
		id: string;
		name: string;
		icon: string;
		state: "idle" | "working" | "error";
		surface: string;
		detail: string;
		ts: number;
	}
	let presence = $state<AgentPresence[]>([]);
	$effect(() => {
		if (!isOpen || !paired || isExchange) {
			presence = [];
			presenceError = "";
			return;
		}
		let gone = false;
		const tick = async () => {
			try {
				const res = await harnessFetch("/agent/status");
				if (!res.ok) throw new Error(`Agent status unavailable (HTTP ${res.status}).`);
				const body = (await res.json()) as { agents?: AgentPresence[] };
				if (!gone && pairedSession()) {
					presence = body.agents ?? [];
					presenceError = "";
				}
			} catch (error) {
				if (!gone) {
					presence = [];
					presenceError = error instanceof Error ? error.message : "The paired harness is unreachable.";
				}
			}
		};
		void tick();
		const timer = setInterval(tick, 2500);
		return () => {
			gone = true;
			clearInterval(timer);
		};
	});
	const agentWorking = $derived(presence.find((p) => p.state === "working"));
	const agentError = $derived(presence.find((p) => p.state === "error" && Date.now() - p.ts < 15 * 60_000));
	const errorHint = $derived(agentError && /auth|key|401|credential/i.test(agentError.detail) ? "Fix in Settings → Agent." : "");

	function who(author: string): string {
		if (author === me) return "You";
		if (author === "scheduler") return "Scheduler";
		// An agent posts as its own object id, wherever it is replying. It is
		// absent from `summaries` (agents are hidden infrastructure), so the
		// name comes from store.agents — without it, a reply on an ordinary
		// object fell through to a raw id fragment.
		const agent = store.agents.find((a) => a.id === author);
		if (agent) return agent.name || "Agent";
		// A chat object's own id: the brain-chat surface.
		if (author === object.id) return object.fields["name"]?.stringValue || "Agent";
		if (author && guestAgents(object.fields).includes(author))
			return store.agents.find((a) => a.id === author)?.name || "Agent";
		return author.slice(0, 6);
	}

	function memberName(endpoint: AgentEndpoint): string {
		if (endpoint.objectId === object.id) {
			const objectName = object.fields["name"]?.stringValue || "This object";
			const agentName = store.agents.find((agent) => agent.id === endpoint.agentId)?.name;
			return agentName && agentName !== objectName ? `${objectName} · ${agentName}` : objectName;
		}
		return endpointName(endpoint);
	}

	/** Discord-style mention pills: `@Agent Name` for any known agent becomes a
	 *  tinted chip. Longest names first so "@Bed Test" wins over a "@Bed" agent.
	 *  Runs on rendered HTML; agent names never appear inside our tags. */
	const escapeHtml = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
	/** Escaped agent name → its icon (emoji, image URL, or "" for none). */
	const mentionIcons = $derived(new Map(store.agents.filter((a) => a.name).map((a) => [escapeHtml(a.name), a.icon])));
	const mentionPattern = $derived.by(() => {
		const names = [...mentionIcons.keys()].sort((a, b) => b.length - a.length);
		if (names.length === 0) return null;
		return new RegExp(`@(${names.map((n) => n.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|")})(?![\\w])`, "g");
	});
	/** The agent's icon as pill HTML: its image, its emoji, or the 🤖 default. */
	function mentionGlyph(name: string): string {
		const icon = mentionIcons.get(name) || objectIcon("", "agent");
		return /^https?:\/\//.test(icon) ? `<img class="mention-ico" src="${escapeHtml(icon).replace(/"/g, "&quot;")}" alt="">` : `<span class="mention-ico">${escapeHtml(icon)}</span>`;
	}
	/** Sent messages: the pill leads with the agent's icon, like Discord's avatar-less role chip. */
	function mentionPills(html: string): string {
		return mentionPattern ? html.replace(mentionPattern, (_, name: string) => `<span class="mention">${mentionGlyph(name)}@${name}</span>`) : html;
	}

	let mirrorEl = $state<HTMLDivElement>();
	/** The draft as HTML for the composer mirror: escaped, mentions pilled.
	 *  The icon is painted over the pill's `@` (which keeps its width but not
	 *  its ink), so the mirror's text never drifts from the textarea's caret.
	 *  A trailing space keeps a final newline's line height in step. */
	function composerMirror(text: string): string {
		const escaped = escapeHtml(text);
		const pilled = mentionPattern
			? escaped.replace(mentionPattern, (_, name: string) => `<span class="mention"><span class="mention-at">@${mentionGlyph(name)}</span>${name}</span>`)
			: escaped;
		return `${pilled} `;
	}

	function startReply(messageId: string, privately = false) {
		replyTo = messageId;
		const target = messageById.get(messageId)?.mailbox?.message;
		privateRecipient = privately && target ? replyRecipients(target, { objectId: object.id, agentId: "" })
			.find((endpoint) => endpoint.objectId === target.sender.objectId)?.objectId ?? "" : "";
		composerEl?.focus();
	}

	function originName(id: string): string {
		return store.summaries.find((s) => s.id === id)?.name || "object";
	}

	/** Stable avatar hue from the author id. */
	function hue(author: string): number {
		let h = 0;
		for (const c of author) h = (h * 31 + c.charCodeAt(0)) % 360;
		return h;
	}

	/**
	 * Emoji avatar for an author, when it has one: agents post as their
	 * object id and carry iconEmoji (set from the space's Agents
	 * settings); the chat object itself covers agent-brain chats.
	 */
	function avatarEmoji(author: string): string {
		const agent = store.agents.find((a) => a.id === author);
		if (agent?.icon) return agent.icon;
		if (author === object.id) return object.fields["iconEmoji"]?.stringValue ?? "";
		return store.summaries.find((s) => s.id === author)?.icon ?? "";
	}

	function when(ts: number): string {
		const d = new Date(ts);
		const today = new Date().toDateString() === d.toDateString();
		return today ? d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) : d.toLocaleDateString();
	}

	async function send() {
		const text = draft.trim();
		const mentions = liveTags(text);
		if (sending || readOnly || !text || (isExchange && !audience.length && !mentions.length)) return;
		const reply = replyTo;
		sending = true;
		sendError = "";
		try {
			// The object's one chat is __discussion__: a line that @-mentions a
			// guest agent is the harness's wake signal, and the agent replies in
			// the same thread - no envelope. Envelopes are for an opened exchange
			// thread (reply-to-all), where they carry the audience.
			if (isExchange) {
				if (!replyMessage) throw new Error("Reload this exchange before replying.");
				const title = privateRecipient ? `Private: ${exchangeTitle}` : exchangeTitle;
				const parentId = reply || replyMessage.id;
				const recipients = uniqueEndpoints([...audience, ...mentions.map((option) => option.endpoint)]);
				const exchangeId = privateRecipient ? crypto.randomUUID() : replyMessage.exchangeId;
				if (!pendingSend || pendingSend.text !== text || pendingSend.replyTo !== parentId || pendingSend.title !== title
					|| pendingSend.requestReply !== requestReply || JSON.stringify(pendingSend.recipients) !== JSON.stringify(recipients)) {
					pendingSend = {
						id: crypto.randomUUID(),
						exchangeId,
						sender: { objectId: object.id, agentId: "" }, recipients,
						text, replyTo: parentId, sentAt: Date.now(), title,
						requestReply, historical: false, operation: "", author: "",
					};
				}
				const sent = await mailbox.send(pendingSend);
				pendingSend = undefined;
				draft = "";
				replyTo = "";
				privateRecipient = "";
				tagged = [];
				await onchanged();
				void seedMentions();
				if (sent.threadId !== threadId) onexchange?.(sent.threadId);
				return;
			}
			await chat.post(object.id, text, reply, threadId === "__discussion__" ? "" : threadId);
			// Cleared only once the change is committed: a failed write used
			// to swallow the message - empty composer, nothing posted, no
			// reason given.
			draft = "";
			replyTo = "";
			tagged = [];
			await onchanged();
			void seedMentions();
		} catch (err) {
			sendError = err instanceof Error ? err.message : String(err);
		} finally {
			sending = false;
		}
	}

	async function retry(messageId: string, stage: "delivery" | "processing", recipientObjectId = "") {
		const key = `${messageId}:${stage}:${recipientObjectId}`;
		if (retrying.includes(key)) return;
		retrying = [...retrying, key];
		retryError = "";
		try {
			await mailbox.retry(object.id, messageId, stage, recipientObjectId || undefined);
			await onchanged();
		} catch (err) {
			retryError = err instanceof Error ? err.message : String(err);
		} finally {
			retrying = retrying.filter((id) => id !== key);
		}
	}

	async function toggleReaction(messageId: string, emoji: string) {
		if (readOnly || isExchange) return;
		pickerFor = "";
		sendError = "";
		try {
			await chat.react(object.id, messageId, emoji);
			await onchanged();
		} catch (err) {
			sendError = err instanceof Error ? err.message : String(err);
		}
	}
</script>

<section class="discussion" class:full class:pagemode>
	{#if !isOpen}
		<!-- Anytype commentCounter: a centered floating pill, not a full-width bar. -->
		<div class="counter-wrap">
			<button class="opener" onclick={openDiscussion}>
				<span class="opener-icon">💬</span>
				<span class="opener-label">{messages.length > 0 ? `${messages.length} comment${messages.length === 1 ? "" : "s"}` : "Start a discussion"}</span>
			</button>
		</div>
	{:else}
		{#if !full}
			<!-- Anytype commentSection: a SHORT rule, then the 600-weight title. -->
			<div class="rule"></div>
			<div class="head">
				<span class="head-title">{isExchange ? exchangeTitle : "Discussion"}</span>
				<button class="collapse" title="Collapse" onclick={() => (open = false)}>×</button>
			</div>
		{/if}
		{#if identityError}
			<p class="presence error" role="status">Cannot load your discussion identity: {identityError}</p>
			<button onclick={() => void loadIdentity()}>Retry discussion identity</button>
		{/if}
		{#if !isExchange && paired && presenceError}
			<p class="presence error" role="status">{presenceError} Check that the paired native app and harness are running. Discussion remains available.</p>
		{/if}
		{#if isExchange && members.length}
			<div class="members" aria-label="Exchange participants">
				<span>Participants</span>
				{#each members as member (member.objectId)}
					<a href="/app/object/{member.objectId}">{memberName(member)}</a>
				{/each}
			</div>
		{:else if isExchange && conversation?.participants.length}
			<p class="presence">Participants: {conversation.participants.map(who).join(", ")}</p>
		{/if}
		{#if legacy}<p class="presence" role="status">This shared exchange is read-only, awaiting migration to object inboxes.</p>{/if}
		{#if isExchange && conversation?.closed}<p class="presence">This exchange is closed.</p>{/if}
		{#if retryError}<p class="presence error" role="alert">Retry failed: {retryError}</p>{/if}
		<div class="messages" bind:this={messagesEl}>
			{#each messages as m (m.id)}
				<div class="msg" class:own={m.author === me} id="msg-{m.id}">
					{#if m.author !== me}
						{@const agentHref = store.agents.some((a) => a.id === m.author) ? `/app/object/${m.author}` : undefined}
						{#if avatarEmoji(m.author)}
							<svelte:element this={agentHref ? "a" : "span"} class="avatar emoji" href={agentHref} title={agentHref ? `Open ${who(m.author)}` : undefined}>{avatarEmoji(m.author)}</svelte:element>
						{:else}
							<svelte:element this={agentHref ? "a" : "span"} class="avatar" href={agentHref} title={agentHref ? `Open ${who(m.author)}` : undefined} style="background: hsl({hue(m.author)}, 45%, 35%)">{m.author.slice(0, 2)}</svelte:element>
						{/if}
					{/if}
					<div class="body">
						{#if m.replyTo && messageById.has(m.replyTo)}
							{@const target = messageById.get(m.replyTo)!}
							<a class="quote" href="#msg-{m.replyTo}">
								<span class="q-author">{who(target.author)}</span>
								<span class="q-text">{target.text.slice(0, 80)}</span>
							</a>
						{/if}
						<div class="meta-row">
							{#if m.author !== me}
								{#if store.agents.some((a) => a.id === m.author)}
									<a class="author agent-link" href="/app/object/{m.author}">{who(m.author)}</a>
								{:else}
									<span class="author">{who(m.author)}</span>
								{/if}
							{/if}
							{#if m.origin}
								<a class="origin" href="/app/object/{m.origin}" title="Asked from this object">↳ {originName(m.origin)}</a>
							{/if}
							<span class="time">{when(m.ts)}</span>
						</div>
						<div class="text md">{@html mentionPills(renderMarkdown(m.text))}</div>
						{#if m.mailbox}
							{@const entry = m.mailbox}
							{@const failedDelivery = entry.outgoing ? entry.deliveries.find((d) => d.status === "failed") : undefined}
							{@const pendingDelivery = entry.outgoing && entry.deliveries.some((d) => d.status === "pending")}
							{@const localWork = entry.incoming && (!!entry.message.operation || entry.message.recipients.some((r) => r.objectId === object.id && r.agentId))}
							{@const waitingFor = entry.message.recipients.filter((r) => r.agentId).map((r) => store.agents.find((a) => a.id === r.agentId)?.name || "the agent").join(", ")}
							<!-- Quiet when all is well (delivered + processed); one line only
							     while in flight or when something needs a hand. -->
							{#if failedDelivery}
								<div class="status failed">
									<span>Not delivered to {memberName(failedDelivery.recipient)}{failedDelivery.error ? ` · ${failedDelivery.error}` : ""}</span>
									<button disabled={retrying.includes(`${m.id}:delivery:${failedDelivery.recipient.objectId}`)} onclick={() => void retry(m.id, "delivery", failedDelivery.recipient.objectId)}>Retry</button>
								</div>
							{:else if localWork && entry.processing.status === "failed"}
								<div class="status failed">
									<span>Failed{entry.processing.error ? ` · ${entry.processing.error}` : ""}</span>
									<button disabled={retrying.includes(`${m.id}:processing:`)} onclick={() => void retry(m.id, "processing")}>Retry</button>
								</div>
							{:else if localWork && entry.processing.status === "held"}
								<div class="status failed"><span>On hold{entry.processing.error ? ` · ${entry.processing.error}` : ""}</span></div>
							{:else if localWork && entry.processing.status === "awaiting_approval"}
								<div class="status"><span>Waiting for approval on the installation's machine</span></div>
							{:else if pendingDelivery}
								<div class="status"><span>Sending…</span></div>
							{:else if localWork && (entry.processing.status === "pending" || entry.processing.status === "processing")}
								<div class="status"><span>Waiting for {waitingFor}…</span></div>
							{/if}
						{/if}
						{#if m.reactions.length > 0 || pickerFor === m.id}
							<div class="reactions">
								{#each m.reactions as r (r.emoji)}
									<button
										class="chip"
										class:mine={r.authors.includes(me)}
										disabled={readOnly || isExchange}
										title={r.authors.map(who).join(", ")}
										onclick={() => void toggleReaction(m.id, r.emoji)}
									>
										{r.emoji} {r.authors.length}
									</button>
								{/each}
							</div>
						{/if}
					</div>
					{#if !readOnly}
						<div class="actions">
							{#if !isExchange}<button title="Add reaction" onclick={() => (pickerFor = pickerFor === m.id ? "" : m.id)}>😀</button>{/if}
							<button title={isExchange ? "Reply to all" : "Reply"} onclick={() => startReply(m.id)}>↩</button>
							{#if m.mailbox && m.mailbox.message.sender.objectId !== object.id}
								<button class="private-reply" title="Reply privately in a separate exchange" onclick={() => startReply(m.id, true)}>Private</button>
							{/if}
						</div>
					{/if}
					{#if pickerFor === m.id}
						<div class="picker-wrap">
							<EmojiPicker onpick={(e) => void toggleReaction(m.id, e)} onclose={() => (pickerFor = "")} />
						</div>
					{/if}
				</div>
			{/each}
			{#if messages.length === 0}
				<p class="empty">No messages yet.</p>
			{/if}
			{#if agentWorking}
				<div class="presence working" title="The agent is composing a reply">
					{#if agentWorking.icon}
						<span class="avatar emoji">{agentWorking.icon}</span>
					{:else}
						<span class="avatar" style="background: hsl({hue(agentWorking.id)}, 45%, 35%)">{agentWorking.name.slice(0, 2)}</span>
					{/if}
					<span class="p-name">{agentWorking.name}</span>
					<span class="dots"><i></i><i></i><i></i></span>
				</div>
			{/if}
			{#if agentError && !agentWorking}
				<div class="presence error">
					<span class="p-glyph">⚠︎</span>
					<span class="p-text">{agentError.name} hit a problem: {agentError.detail}{errorHint ? ` — ${errorHint}` : ""}</span>
				</div>
			{/if}
		</div>
		{#if !readOnly && replyTo && messageById.has(replyTo)}
			{@const target = messageById.get(replyTo)!}
			<div class="replying">
				<span class="q-author">Replying to {who(target.author)}</span>
				<span class="q-text">{target.text.slice(0, 60)}</span>
				<button title="Cancel reply" onclick={() => { replyTo = ""; privateRecipient = ""; }}>×</button>
			</div>
		{/if}
		{#if sendError}
			<p class="presence error" role="alert">{sendError}</p>
		{/if}
		{#if !readOnly}
		{#if isExchange}
			<div class="audience">
				<label>Reply audience
					<select bind:value={privateRecipient} disabled={sending}>
						<option value="">Reply to all ({replyAll.length})</option>
						{#each replyAll as recipient (recipient.objectId)}
							<option value={recipient.objectId}>Private · {memberName(recipient)}</option>
						{/each}
					</select>
				</label>
				<span>{privateRecipient ? "New private exchange. Only this recipient receives your message." : `To ${audience.map(memberName).join(", ") || "no recipients"}`}</span>
				<label class="request-reply"><input type="checkbox" bind:checked={requestReply} disabled={sending} /> Ask agents to respond</label>
			</div>
		{/if}
		<!-- Anytype commentForm: rounded highlight box, content area on top,
		     toolbar row with the send control at the right. -->
		<div class="composer">
			{#if mention && mentionMatches.length}
				<div class="mention-menu" role="listbox" aria-label="Tag an agent">
					{#each mentionMatches as option, i (option.endpoint.objectId)}
						<button
							type="button"
							role="option"
							aria-selected={i === mentionIndex}
							class:active={i === mentionIndex}
							onmouseenter={() => (mentionIndex = i)}
							onmousedown={(e) => { e.preventDefault(); pickMention(option); }}
						>
							{#if option.icon}
								<span class="m-icon">{option.icon}</span>
							{:else}
								<span class="m-icon m-dot" style="background: hsl({hue(option.endpoint.agentId)}, 45%, 35%)">{option.agentName.slice(0, 2)}</span>
							{/if}
							<span class="m-name">@{option.agentName}</span>
							{#if option.name !== option.agentName}<span class="m-where">{option.name}</span>{/if}
						</button>
					{/each}
				</div>
			{:else if mention && tagOptions && !mentionMatches.length}
				<div class="mention-menu empty">No agent matches “{mention.query}”.</div>
			{/if}
			<!-- A textarea cannot style part of its text, so a mirror behind it
			     draws the draft with mention pills; the textarea's own glyphs
			     are transparent and only its caret and selection show. -->
			<div class="input-wrap">
			<div class="mirror" aria-hidden="true" bind:this={mirrorEl}>{@html composerMirror(draft)}</div>
			<textarea
				bind:this={composerEl}
				placeholder={isExchange ? "Write a message… (@ to tag an agent)" : "Write a comment… (@ to tag an agent)"}
				bind:value={draft}
				rows={1}
				disabled={sending}
				onscroll={(e) => { if (mirrorEl) mirrorEl.scrollTop = e.currentTarget.scrollTop; }}
				oninput={(e) => {
					const el = e.currentTarget as HTMLTextAreaElement;
					el.style.height = "auto";
					el.style.height = `${el.scrollHeight}px`;
					updateMention();
				}}
				onclick={updateMention}
				onkeyup={(e) => { if (e.key.startsWith("Arrow") && !mention) updateMention(); }}
				onblur={() => { mention = null; }}
				onkeydown={(e) => {
					if (mention && mentionMatches.length) {
						if (e.key === "ArrowDown") { e.preventDefault(); mentionIndex = (mentionIndex + 1) % mentionMatches.length; return; }
						if (e.key === "ArrowUp") { e.preventDefault(); mentionIndex = (mentionIndex + mentionMatches.length - 1) % mentionMatches.length; return; }
						if (e.key === "Enter" || e.key === "Tab") { e.preventDefault(); pickMention(mentionMatches[Math.min(mentionIndex, mentionMatches.length - 1)]); return; }
						if (e.key === "Escape") { e.preventDefault(); mention = null; return; }
					}
					if (e.key === "Enter" && !e.shiftKey) {
						e.preventDefault();
						void send();
					}
					if (e.key === "Escape") { e.preventDefault(); replyTo = ""; privateRecipient = ""; }
				}}
			></textarea>
			</div>
			<div class="form-toolbar">
				<span class="toolbar-side"></span>
				<button class="send" disabled={sending || !draft.trim() || (isExchange && !audience.length)} aria-label="Send" onclick={() => void send()}>
					<svg viewBox="0 0 20 20" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M10 16V5M10 5L5 10M10 5l5 5" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" /></svg>
				</button>
			</div>
		</div>
		{/if}
	{/if}
</section>

<style>
	/* Anytype commentSection: the whole section (divider, title, posts,
	   composer) lives inside the content column - align to the 48px rail. */
	.discussion {
		margin-top: 32px;
		margin-left: 48px;
	}
	.discussion.full {
		margin-left: 0;
	}
	.counter-wrap {
		display: flex;
		justify-content: center;
		padding: 4px 0 12px;
	}
	/* Anytype's short section rule — not full width. */
	/* Anytype commentSection.isVisible: a full-width border-top across
	   the section (comment.scss:40), aligned with the content column. */
	.rule {
		width: 100%;
		height: 1px;
		background: var(--border);
		margin: 0 0 16px;
	}
	.discussion.full {
		margin-top: 8px;
		border-top: none;
		display: flex;
		flex-direction: column;
		flex: 1;
		min-height: 60vh;
	}
	.discussion.full .messages {
		max-height: none;
		flex: 1;
	}
	.origin {
		font-size: 11px;
		color: var(--accent);
		text-decoration: none;
		border: 1px solid var(--border);
		border-radius: 6px;
		padding: 0 6px;
	}
	.origin:hover {
		border-color: var(--accent);
	}
	.opener {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		border: none;
		background: rgb(255 255 255 / 0.06);
		backdrop-filter: blur(10px);
		color: var(--muted);
		font-size: 12px;
		font-weight: 500;
		cursor: pointer;
		padding: 7px 12px 7px 10px;
		border-radius: 18px;
	}
	/* Mobile: the opener collapses to a centered round chat button. */
	@media (max-width: 720px) {
		.discussion {
			/* The desktop 48px rail margin skewed the centered opener
			   24px right of the true viewport middle. */
			margin-left: 0;
			padding: 0 16px;
		}
		.counter-wrap {
			display: flex;
			justify-content: center;
		}
		.opener {
			width: 48px;
			height: 48px;
			border-radius: 50%;
			padding: 0;
			justify-content: center;
			font-size: 20px;
		}
		.opener .opener-icon {
			margin: 0;
		}
		.opener .opener-label {
			display: none;
		}
	}
	.opener:hover {
		background: var(--hover);
		color: var(--fg);
	}
	.opener-icon {
		font-size: 12px;
	}
	.head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		margin-bottom: 8px;
	}
	.head-title {
		font-size: 11px;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--muted);
	}
	.collapse {
		border: none;
		background: none;
		color: var(--muted);
		cursor: pointer;
		font-size: 14px;
	}
	.messages {
		display: flex;
		flex-direction: column;
		gap: 14px;
		padding: 4px 0;
	}
	.msg {
		display: flex;
		gap: 10px;
		position: relative;
		align-items: flex-end;
	}
	/* Own messages ride the right edge, iMessage style: no avatar, no
	   name, accent bubble; everyone else keeps the left column. */
	.msg.own {
		flex-direction: row-reverse;
	}
	.msg .actions {
		opacity: 0;
		display: flex;
		gap: 2px;
		align-self: flex-start;
		transition: opacity 0.1s;
	}
	.msg:hover .actions {
		opacity: 1;
	}
	.msg .actions button {
		border: none;
		background: none;
		cursor: pointer;
		font-size: 13px;
		padding: 2px 4px;
		border-radius: 6px;
	}
	.msg .actions button:hover {
		background: var(--hover);
	}
	.avatar {
		flex: none;
		width: 28px;
		height: 28px;
		border-radius: 50%;
		display: flex;
		align-items: center;
		justify-content: center;
		font-size: 10px;
		font-family: ui-monospace, monospace;
		color: #fff;
	}
	a.avatar {
		text-decoration: none;
		cursor: pointer;
	}
	a.avatar:hover {
		box-shadow: 0 0 0 2px var(--accent);
	}
	.body {
		min-width: 0;
		max-width: 78%;
		display: flex;
		flex-direction: column;
		align-items: flex-start;
	}
	.msg.own .body {
		align-items: flex-end;
	}
	.meta-row {
		display: flex;
		align-items: baseline;
		gap: 8px;
	}
	.author {
		font-size: 12px;
		font-weight: 600;
	}
	.author.agent-link {
		color: var(--fg);
		text-decoration: none;
	}
	.author.agent-link:hover {
		text-decoration: underline;
	}
	.time {
		font-size: 11px;
		color: var(--muted);
	}
	.text {
		font-size: 14px;
		line-height: 1.45;
		word-break: break-word;
		background: var(--hover, #2a2a2e);
		padding: 7px 12px;
		border-radius: 16px;
		border-bottom-left-radius: 5px;
	}
	.msg.own .text {
		background: var(--accent, #0a84ff);
		color: #fff;
		border-radius: 16px;
		border-bottom-right-radius: 5px;
	}
	.msg.own .md :global(a) {
		color: #fff;
		text-decoration: underline;
	}
	.msg.own .md :global(code.ic) {
		background: rgba(255, 255, 255, 0.18);
	}
	.msg.own .meta-row {
		justify-content: flex-end;
	}
	.msg.own .reactions {
		justify-content: flex-end;
	}
	/* Markdown render ({@html} content needs :global under scoped styles) */
	.md :global(ul.md-tasks) {
		list-style: none;
		padding-left: 2px;
		margin: 2px 0;
	}
	.md :global(li.md-task) {
		display: flex;
		align-items: baseline;
		gap: 7px;
	}
	.md :global(.md-cb) {
		flex: none;
		width: 13px;
		height: 13px;
		border: 1.5px solid currentColor;
		opacity: 0.7;
		border-radius: 4px;
		transform: translateY(2px);
		display: inline-flex;
		align-items: center;
		justify-content: center;
		font-size: 10px;
		line-height: 1;
	}
	.md :global(li.md-task.md-done > span:last-child) {
		opacity: 0.6;
		text-decoration: line-through;
	}
	.md :global(p) {
		margin: 0 0 6px;
	}
	.md :global(p:last-child) {
		margin-bottom: 0;
	}
	.md :global(code.ic) {
		font-family: ui-monospace, monospace;
		font-size: 12.5px;
		background: var(--hover, #2a2a2a);
		border-radius: 4px;
		padding: 1px 5px;
	}
	.md :global(pre.cb) {
		position: relative;
		font-family: ui-monospace, monospace;
		font-size: 12.5px;
		line-height: 1.5;
		background: var(--hover, #1d1d1d);
		border: 1px solid var(--border, #2a2a2a);
		border-radius: 8px;
		padding: 8px 10px;
		margin: 6px 0;
		overflow-x: auto;
		white-space: pre;
	}
	.md :global(pre.cb .cb-lang) {
		display: block;
		font-size: 10px;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		color: var(--muted);
		margin-bottom: 4px;
	}
	.md :global(pre.cb code) {
		background: none;
		padding: 0;
	}
	.md :global(ul),
	.md :global(ol) {
		margin: 4px 0;
		padding-left: 20px;
	}
	.md :global(blockquote) {
		margin: 4px 0;
		padding: 2px 10px;
		border-left: 2px solid var(--accent);
		color: var(--muted);
	}
	.md :global(.md-h) {
		font-weight: 700;
		margin: 6px 0 2px;
	}
	.md :global(.md-h1) {
		font-size: 17px;
	}
	.md :global(.md-h2) {
		font-size: 15.5px;
	}
	.md :global(.md-h3) {
		font-size: 14px;
	}
	.md :global(hr) {
		border: none;
		border-top: 1px solid var(--border, #2a2a2a);
		margin: 8px 0;
	}
	.md :global(a) {
		color: var(--accent);
	}
	.quote {
		display: flex;
		gap: 6px;
		align-items: baseline;
		font-size: 11px;
		color: var(--muted);
		border-left: 2px solid var(--accent);
		padding: 1px 6px;
		margin-bottom: 2px;
		text-decoration: none;
		overflow: hidden;
		white-space: nowrap;
	}
	.quote .q-text {
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.q-author {
		font-weight: 600;
		flex: none;
	}
	.reactions {
		display: flex;
		flex-wrap: wrap;
		gap: 4px;
		margin-top: 4px;
	}
	.chip {
		border: 1px solid var(--border);
		background: none;
		color: inherit;
		border-radius: 999px;
		padding: 1px 8px;
		font-size: 12px;
		cursor: pointer;
	}
	.chip:hover {
		border-color: var(--accent);
	}
	.chip.mine {
		border-color: var(--accent);
		background: rgb(10 132 255 / 0.12);
	}
	.picker-wrap {
		position: absolute;
		right: 0;
		top: 24px;
		z-index: 95;
	}
	.replying {
		display: flex;
		align-items: baseline;
		gap: 8px;
		font-size: 11px;
		color: var(--muted);
		border-left: 2px solid var(--accent);
		padding: 2px 8px;
		margin: 8px 0 4px;
		overflow: hidden;
		white-space: nowrap;
	}
	.replying button {
		border: none;
		background: none;
		color: var(--muted);
		cursor: pointer;
		margin-left: auto;
	}
	/* Anytype commentForm: rounded shape-highlight-light box. */
	.composer {
		position: relative;
		display: flex;
		flex-direction: column;
		background: var(--panel);
		border: 1px solid var(--border);
		border-radius: 10px;
		margin-top: 10px;
	}
	.mention-menu {
		position: absolute;
		bottom: 100%;
		left: 0;
		right: 0;
		margin-bottom: 6px;
		background: var(--panel);
		border: 1px solid var(--border);
		border-radius: 10px;
		box-shadow: 0 6px 24px rgba(0, 0, 0, 0.35);
		padding: 4px;
		display: flex;
		flex-direction: column;
		max-height: 260px;
		overflow-y: auto;
		z-index: 30;
		font-size: 13px;
		color: var(--muted);
	}
	.mention-menu.empty { padding: 10px 12px; }
	.mention-menu button {
		display: flex;
		align-items: center;
		gap: 8px;
		width: 100%;
		padding: 7px 8px;
		background: none;
		border: none;
		border-radius: 6px;
		color: var(--fg);
		font: inherit;
		text-align: left;
		cursor: pointer;
	}
	.mention-menu button.active { background: var(--border); }
	.mention-menu .m-icon {
		width: 20px;
		height: 20px;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		border-radius: 50%;
		font-size: 13px;
		flex: none;
	}
	.mention-menu .m-dot { color: #fff; font-size: 9px; text-transform: uppercase; }
	.mention-menu .m-name { font-weight: 500; }
	.mention-menu .m-where { color: var(--muted); font-size: 11px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.input-wrap {
		position: relative;
		display: flex;
		flex-direction: column;
	}
	/* Mirror and textarea share every metric that affects wrapping, so the
	   pills sit exactly under the (invisible) glyphs the caret moves through. */
	.composer textarea,
	.mirror {
		font: inherit;
		font-size: 14px;
		line-height: 1.45;
		letter-spacing: normal;
		padding: 12px 12px 4px;
		white-space: pre-wrap;
		overflow-wrap: break-word;
		word-break: normal;
		scrollbar-gutter: stable;
		box-sizing: border-box;
	}
	.mirror {
		position: absolute;
		inset: 0;
		color: var(--fg);
		overflow: hidden;
		pointer-events: none;
	}
	.composer textarea {
		position: relative;
		background: none;
		border: none;
		outline: none;
		resize: none;
		color: transparent;
		caret-color: var(--fg);
		max-height: 40vh;
		overflow-y: auto;
	}
	.composer textarea::placeholder {
		color: var(--muted);
	}
	.composer textarea::selection {
		background: rgb(10 132 255 / 0.35);
		color: transparent;
	}
	/* The composer pill cannot take padding or weight (either would widen the
	   text under the caret); the shadow paints the breathing room instead. */
	.mirror :global(.mention) {
		background: rgba(88, 101, 242, 0.3);
		/* Height only: a side ring would cover the ~4px space between adjacent tags. */
		box-shadow: 0 2px 0 0 rgba(88, 101, 242, 0.3), 0 -2px 0 0 rgba(88, 101, 242, 0.3);
		color: #c9cdfb;
		border-radius: 3px;
	}
	/* The `@` holds its width for the caret; the agent's icon covers it. */
	.mirror :global(.mention-at) {
		position: relative;
		color: transparent;
	}
	/* Drawn inside the hidden `@`'s width (the caret depends on it), but
	   smaller and to the left, so a gap stays between icon and name. */
	.mirror :global(.mention-at .mention-ico) {
		position: absolute;
		left: 1px;
		top: 50%;
		transform: translateY(-50%);
		width: 1em;
		height: 1em;
		font-size: 0.68em;
		line-height: 1em;
		text-align: center;
		color: var(--fg);
		border-radius: 3px;
		object-fit: cover;
	}
	.form-toolbar {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 4px 8px 8px;
		min-height: 36px;
	}
	.send {
		width: 24px;
		height: 24px;
		border-radius: 50%;
		border: none;
		background: var(--accent);
		color: #fff;
		cursor: pointer;
		display: flex;
		align-items: center;
		justify-content: center;
		padding: 0;
	}
	.send :global(svg) {
		width: 16px;
		height: 16px;
	}
	.send:disabled {
		opacity: 0.35;
		cursor: default;
	}
	.send:not(:disabled):hover {
		border-color: var(--accent);
	}
	.empty {
		color: var(--muted);
		font-size: 13px;
	}
	.avatar.emoji {
		background: var(--hl-light, rgba(255, 255, 255, 0.07));
		font-size: 17px;
	}
	.presence {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 2px 0 6px;
		font-size: 12px;
		color: var(--muted);
	}
	.presence .p-name {
		font-weight: 500;
	}
	.dots {
		display: inline-flex;
		gap: 3px;
		align-items: center;
	}
	.dots i {
		width: 5px;
		height: 5px;
		border-radius: 50%;
		background: var(--muted);
		animation: dot-pulse 1.2s infinite ease-in-out;
	}
	.dots i:nth-child(2) {
		animation-delay: 0.2s;
	}
	.dots i:nth-child(3) {
		animation-delay: 0.4s;
	}
	@keyframes dot-pulse {
		0%, 60%, 100% { opacity: 0.25; transform: translateY(0); }
		30% { opacity: 1; transform: translateY(-2px); }
	}
	.presence.error {
		color: var(--orange, #ff9f0a);
	}
	.presence .p-glyph {
		font-size: 13px;
	}
	.presence .p-text {
		line-height: 1.4;
	}
	/* Full-page chat (mobile route): fill the shell, composer at the
	   bottom, messages take the rest. */
	.discussion.pagemode {
		height: 100%;
		min-height: 0;
		display: flex;
		flex-direction: column;
	}
	.pagemode .messages {
		flex: 1;
		min-height: 0;
		max-height: none;
		overflow-y: auto;
	}
	.pagemode .composer {
		flex: none;
	}
	.members, .audience, .status {
		font-size: 11px;
		color: var(--muted);
		line-height: 1.5;
		overflow-wrap: anywhere;
	}
	.members { display: flex; flex-wrap: wrap; gap: 5px 8px; padding: 8px 0; }
	.members a { color: var(--fg); text-decoration: none; }
	.status { display: flex; flex-wrap: wrap; align-items: baseline; gap: 3px 6px; padding: 2px 2px 0; }
	.status.failed { color: var(--orange, #ff9f0a); }
	.status button {
		background: none;
		border: 1px solid var(--border);
		border-radius: 5px;
		color: inherit;
		cursor: pointer;
		font: inherit;
		padding: 1px 5px;
	}
	.status button:disabled { opacity: 0.5; }
	/* Discord-style mention: tinted pill, lighter text, slight weight. */
	.text :global(.mention) {
		background: rgba(88, 101, 242, 0.3);
		color: #c9cdfb;
		border-radius: 3px;
		padding: 0 2px;
		font-weight: 500;
	}
	/* On the solid accent bubble: the same light touch - a faint blurple
	   wash and lavender-white text, no heavier weight. */
	.msg.own .text :global(.mention) {
		background: rgba(88, 101, 242, 0.45);
		color: #eef0ff;
	}
	.text :global(.mention-ico) {
		display: inline-block;
		margin-right: 3px;
		font-size: 0.9em;
		line-height: 1;
	}
	.text :global(img.mention-ico) {
		width: 1em;
		height: 1em;
		border-radius: 3px;
		vertical-align: -0.12em;
		object-fit: cover;
	}
	.audience { margin-top: 10px; display: flex; flex-direction: column; gap: 5px; }
	.audience label { display: flex; align-items: center; gap: 7px; }
	.audience select {
		min-width: 0;
		max-width: 100%;
		background: var(--panel);
		color: var(--fg);
		border: 1px solid var(--border);
		border-radius: 5px;
		padding: 4px;
		font: inherit;
	}
	.msg .actions .private-reply { color: var(--muted); font-size: 11px; }
</style>
