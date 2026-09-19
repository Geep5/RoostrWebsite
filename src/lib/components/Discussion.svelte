<script lang="ts">
	/** Human/private chat stays local; exchanges send immutable mailbox envelopes. */
	import type { AgentEndpoint, AgentMessage, ObjectJSON } from "$lib/types";
	import { chatMessages, isLegacyExchange, replyRecipients, uniqueEndpoints } from "$lib/chat";
	import { endpointName } from "$lib/conversations";
	import { goto } from "$app/navigation";
	import { chat, mailbox, settings } from "$lib/api";
	import { store } from "$lib/data.svelte";
	import EmojiPicker from "./EmojiPicker.svelte";
	import { renderMarkdown } from "$lib/markdown";
	import { onMount } from "svelte";
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

	const messages = $derived(chatMessages(object, threadId));
	const conversation = $derived(object.conversations?.find((c) => c.id === threadId));
	const legacy = $derived(isLegacyExchange(object, threadId));
	const isExchange = $derived(legacy || conversation?.kind === "a2a" || messages.some((m) => m.mailbox));
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
	let draft = $state("");
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
	const replyMessage = $derived((replyTo ? messageById.get(replyTo) : messages[messages.length - 1])?.mailbox?.message);
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
		if (author && author === object.fields["agent"]?.stringValue)
			return object.fields["name"]?.stringValue || "Agent";
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
		if (sending || readOnly || !text || (isExchange && !audience.length)) return;
		const reply = replyTo;
		sending = true;
		sendError = "";
		try {
			if (isExchange) {
				if (!replyMessage) throw new Error("Reload this exchange before replying.");
				const title = privateRecipient ? `Private: ${exchangeTitle}` : exchangeTitle;
				const parentId = reply || replyMessage.id;
				const recipients = audience;
				if (!pendingSend || pendingSend.text !== text || pendingSend.replyTo !== parentId || pendingSend.title !== title
					|| pendingSend.requestReply !== requestReply || JSON.stringify(pendingSend.recipients) !== JSON.stringify(recipients)) {
					pendingSend = {
						id: crypto.randomUUID(),
						exchangeId: privateRecipient ? crypto.randomUUID() : replyMessage.exchangeId,
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
				await onchanged();
				if (sent.threadId !== threadId) onexchange?.(sent.threadId);
				return;
			}
			await chat.post(object.id, text, reply, threadId === "__discussion__" ? "" : threadId);
			// Cleared only once the change is committed: a failed write used
			// to swallow the message - empty composer, nothing posted, no
			// reason given.
			draft = "";
			replyTo = "";
			await onchanged();
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
		{#if !isExchange && !paired}
			<p class="presence">Pair under This machine to see live agent status. Discussion history and comments remain available.</p>
		{:else if !isExchange && presenceError}
			<p class="presence error" role="status">{presenceError} Check that the paired native app and harness are running. Discussion remains available.</p>
		{/if}
		{#if members.length}
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
						{#if avatarEmoji(m.author)}
							<span class="avatar emoji">{avatarEmoji(m.author)}</span>
						{:else}
							<span class="avatar" style="background: hsl({hue(m.author)}, 45%, 35%)">{m.author.slice(0, 2)}</span>
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
								<span class="author">{who(m.author)}</span>
							{/if}
							{#if m.origin}
								<a class="origin" href="/app/object/{m.origin}" title="Asked from this object">↳ {originName(m.origin)}</a>
							{/if}
							<span class="time">{when(m.ts)}</span>
						</div>
						<div class="text md">{@html renderMarkdown(m.text)}</div>
						{#if m.mailbox}
							{@const entry = m.mailbox}
							<div class="receipts">
								<span>To {entry.message.recipients.map(memberName).join(", ")}</span>
								{#if entry.outgoing}
									{#each entry.deliveries as delivery (delivery.recipient.objectId)}
										<div class="receipt" class:failed={delivery.status === "failed"}>
											<span>{memberName(delivery.recipient)} · {delivery.status}{delivery.status === "delivered" ? " to inbox" : ""}</span>
											{#if delivery.error}<span>{delivery.error}</span>{/if}
											{#if delivery.status === "failed"}
												<button disabled={retrying.includes(`${m.id}:delivery:${delivery.recipient.objectId}`)} onclick={() => void retry(m.id, "delivery", delivery.recipient.objectId)}>Retry delivery</button>
											{/if}
										</div>
									{/each}
								{/if}
								{#if entry.incoming && (entry.message.operation || entry.message.recipients.some((recipient) => recipient.objectId === object.id && recipient.agentId))}
									<div class="receipt" class:failed={entry.processing.status === "failed"}>
										<span>Local processing · {entry.processing.status.replaceAll("_", " ")}</span>
										{#if entry.processing.error}<span>{entry.processing.error}</span>{/if}
										{#if entry.processing.status === "failed"}
											<button disabled={retrying.includes(`${m.id}:processing:`)} onclick={() => void retry(m.id, "processing")}>Retry processing</button>
										{:else if entry.processing.status === "awaiting_approval"}
											<span>Approve on the installation's owning machine.</span>
										{/if}
									</div>
								{/if}
							</div>
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
			<textarea
				bind:this={composerEl}
				placeholder={isExchange ? "Write a message… (markdown supported)" : "Write a comment… (markdown supported)"}
				bind:value={draft}
				rows={1}
				disabled={sending}
				oninput={(e) => {
					const el = e.currentTarget as HTMLTextAreaElement;
					el.style.height = "auto";
					el.style.height = `${el.scrollHeight}px`;
				}}
				onkeydown={(e) => {
					if (e.key === "Enter" && !e.shiftKey) {
						e.preventDefault();
						void send();
					}
					if (e.key === "Escape") { e.preventDefault(); replyTo = ""; privateRecipient = ""; }
				}}
			></textarea>
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
		display: flex;
		flex-direction: column;
		background: var(--panel);
		border: 1px solid var(--border);
		border-radius: 10px;
		margin-top: 10px;
	}
	.composer textarea {
		background: none;
		border: none;
		outline: none;
		resize: none;
		color: var(--fg);
		font: inherit;
		font-size: 14px;
		line-height: 1.45;
		padding: 12px 12px 4px;
		max-height: 40vh;
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
	.members, .audience, .receipts {
		font-size: 11px;
		color: var(--muted);
		line-height: 1.5;
		overflow-wrap: anywhere;
	}
	.members { display: flex; flex-wrap: wrap; gap: 5px 8px; padding: 8px 0; }
	.members a { color: var(--fg); text-decoration: none; }
	.receipts { display: flex; flex-direction: column; gap: 3px; padding: 3px 2px; }
	.receipt { display: flex; flex-wrap: wrap; align-items: baseline; gap: 3px 6px; }
	.receipt.failed { color: var(--orange, #ff9f0a); }
	.receipt button {
		background: none;
		border: 1px solid var(--border);
		border-radius: 5px;
		color: inherit;
		cursor: pointer;
		font: inherit;
		padding: 1px 5px;
	}
	.receipt button:disabled { opacity: 0.5; }
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
