/**
 * Capability requests, staged and resolved from a capability's page. The DAG
 * carries only the intent - a mailbox message on the capability object;
 * execution waits for a paired human's approval on the computer that serves
 * it (its `served_by`).
 */

import { fetchAllQuery, mailbox } from "$lib/api";
import { harnessFetch } from "$lib/local-transport";
import type { BadgeIcon } from "$lib/options";
import { fieldStr } from "$lib/types";

/** A capability's status in words, as its serving computer's harness wrote it. */
export function capabilityStatusText(status: string): string {
	switch (status || "missing") {
		case "active": return "Active";
		case "needs_auth": return "Needs sign-in";
		case "needs_approval": return "Awaiting approval";
		case "processing": return "Working…";
		case "disabled": return "Switched off";
		case "broken": return "Broken";
		case "missing": return "Not installed";
		default: return status.replaceAll("_", " ");
	}
}

/** The status row's badge: glyph and options.ts colour name. */
export function capabilityStatusBadge(status: string): { icon: BadgeIcon; color: string } {
	switch (status || "missing") {
		case "active": return { icon: "check", color: "lime" };
		case "needs_auth":
		case "needs_approval":
		case "processing": return { icon: "clock", color: "orange" };
		case "broken": return { icon: "x", color: "red" };
		default: return { icon: "dashed", color: "" };
	}
}

export interface CapabilityRequest {
	/** The capability object the request was staged on. */
	objectId: string;
	messageId: string;
	key: string;
	operation: string;
	sender: { objectId: string; agentId: string };
	status: string;
	error: string;
	sentAt: number;
	canApprove: boolean;
}

/** Requests waiting on a human or in flight, harness-side view. */
export async function listCapabilityRequests(): Promise<CapabilityRequest[]> {
	const res = await harnessFetch("/capability-requests");
	if (!res.ok) throw new Error(`Cannot load approvals (HTTP ${res.status}).`);
	return ((await res.json()) as { requests: CapabilityRequest[] }).requests;
}

/** This machine's stable id ("" when the paired harness is unreachable). */
export async function thisMachineId(): Promise<string> {
	try {
		const res = await harnessFetch("/machine");
		if (!res.ok) return "";
		return ((await res.json()) as { id: string }).id;
	} catch {
		return "";
	}
}

/**
 * Stage one operation into a capability's mailbox, sent by this machine's
 * own machine object. Only the computer that serves the capability may
 * stage on it: the request runs there after a paired approval. Throws with
 * a human-readable reason; the same open request is not sent twice.
 */
export async function stageCapabilityRequest(capabilityId: string, key: string, operation: string): Promise<void> {
	const id = await thisMachineId();
	if (!id) throw new Error("Cannot identify this computer. Is the harness running?");
	const source = (await fetchAllQuery({ type: "machine" })).find((row) => fieldStr(row.fields, "machine_id") === id);
	if (!source) throw new Error("This computer has not published its object yet.");
	const open = await listCapabilityRequests().catch(() => [] as CapabilityRequest[]);
	if (open.some((r) => r.objectId === capabilityId && r.operation === operation && OPEN_REQUEST.includes(r.status))) {
		throw new Error("This request is already waiting for approval.");
	}
	await mailbox.send({ id: crypto.randomUUID(), exchangeId: crypto.randomUUID(), sender: { objectId: source.id, agentId: "" }, recipients: [{ objectId: capabilityId, agentId: "" }], text: `Request ${operation} for ${key}.`, replyTo: "", sentAt: Date.now(), title: "Capability request", requestReply: true, historical: false, operation, author: "" });
}

/** Request statuses still waiting on a human or running. */
export const OPEN_REQUEST = ["pending", "awaiting_approval", "processing"];

/** Approve or reject a waiting request. Approval executes on this machine's harness. */
export async function resolveCapabilityRequest(request: CapabilityRequest, action: "approve" | "reject"): Promise<void> {
	const body = { objectId: request.objectId, messageId: request.messageId };
	const res = await harnessFetch(`/capability-requests/${action}`, { method: "POST", body: JSON.stringify(body) });
	const result = (await res.json().catch(() => ({}))) as { error?: string };
	if (!res.ok || result.error) throw new Error(result.error ?? `HTTP ${res.status}`);
}
