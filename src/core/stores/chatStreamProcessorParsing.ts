import type { ParsedStreamEvent } from "./chatStreamProcessorSupport";
import {
  makeConversationIdentity,
  makeMessageIdentity,
  messageIdentityKey,
  topicIdentityKey,
  type ConversationOwnerType,
} from "./chatStoreIdentity";
import {
  hasInvalidRequiredAlias,
  readStreamGeneration,
} from "./chatStreamProcessorAliases";

export function firstDefined(...values: unknown[]): unknown {
  return values.find((value) => value !== undefined && value !== null);
}

interface StreamEventFields {
  context: Record<string, any>;
  messageId: string;
  topicId: string;
  generation: number;
  streamGeneration: number | null;
}

const ALIAS_CONFLICT = Symbol("alias-conflict");

type AliasResolution = unknown | typeof ALIAS_CONFLICT | undefined;

function isNonEmptyAlias(value: unknown): boolean {
  if (value === undefined || value === null) return false;
  return typeof value !== "string" || value.trim().length > 0;
}

function resolveAlias(
  event: any,
  context: Record<string, any>,
  keys: string[],
): AliasResolution {
  const values = keys.flatMap((key) => {
    const snake = key.replace(/[A-Z]/g, (letter) => `_${letter.toLowerCase()}`);
    return [context[key], context[snake], event[key], event[snake]];
  });
  const present = values.filter(isNonEmptyAlias);
  if (present.length === 0) return undefined;
  const first = present[0];
  return present.every((value) => value === first) ? first : ALIAS_CONFLICT;
}

function readStreamEventFields(event: any): StreamEventFields | null {
  if (!event || typeof event !== "object") return null;
  const context =
    event.context && typeof event.context === "object"
      ? (event.context as Record<string, any>)
      : {};
  if (
    hasInvalidRequiredAlias(event, context, [
      "messageId",
      "msgId",
      "requestId",
      "topicId",
    ])
  )
    return null;
  const messageAlias = resolveAlias(event, context, ["messageId", "msgId"]);
  const requestAlias = resolveAlias(event, context, ["requestId"]);
  if (
    messageAlias === ALIAS_CONFLICT ||
    requestAlias === ALIAS_CONFLICT ||
    (isNonEmptyAlias(messageAlias) &&
      isNonEmptyAlias(requestAlias) &&
      messageAlias !== requestAlias)
  )
    return null;
  const messageId = isNonEmptyAlias(messageAlias) ? messageAlias : requestAlias;
  const topicId = resolveAlias(event, context, ["topicId"]);
  if (topicId === ALIAS_CONFLICT) return null;
  if (
    typeof messageId !== "string" ||
    !messageId ||
    typeof topicId !== "string" ||
    !topicId
  )
    return null;
  const generation = readStreamGeneration(event, context);
  if (generation === null) return null;
  const streamGeneration =
    typeof event.streamGeneration === "number" &&
    Number.isSafeInteger(event.streamGeneration) &&
    event.streamGeneration > 0
      ? event.streamGeneration
      : null;
  return { context, messageId, topicId, generation, streamGeneration };
}

interface StreamOwnerFields {
  explicitOwnerType: unknown;
  ownerId: unknown;
  groupId: unknown;
  agentId: unknown;
  groupFlag: unknown;
}

function readStreamOwnerFields(
  event: any,
  context: Record<string, any>,
): StreamOwnerFields | null {
  if (hasInvalidRequiredAlias(event, context, ["ownerType", "ownerId"]))
    return null;
  const explicitOwnerType = resolveAlias(event, context, ["ownerType"]);
  const ownerId = resolveAlias(event, context, ["ownerId"]);
  const groupId = resolveAlias(event, context, ["groupId"]);
  const agentId = resolveAlias(event, context, ["agentId"]);
  const groupFlag = resolveAlias(event, context, ["isGroupMessage"]);
  if (
    explicitOwnerType === ALIAS_CONFLICT ||
    ownerId === ALIAS_CONFLICT ||
    groupId === ALIAS_CONFLICT ||
    agentId === ALIAS_CONFLICT ||
    groupFlag === ALIAS_CONFLICT
  )
    return null;
  return {
    explicitOwnerType,
    ownerId,
    groupId,
    agentId,
    groupFlag,
  };
}

interface ResolvedStreamOwner {
  ownerType: ConversationOwnerType;
  ownerId: string;
}

function resolveStreamOwner(
  fields: StreamOwnerFields,
): ResolvedStreamOwner | null {
  const { explicitOwnerType, ownerId, groupId, agentId, groupFlag } = fields;
  if (
    explicitOwnerType !== undefined &&
    explicitOwnerType !== "agent" &&
    explicitOwnerType !== "group"
  )
    return null;

  if (groupFlag !== undefined && typeof groupFlag !== "boolean") return null;
  if (groupId !== undefined && agentId !== undefined) return null;
  if (
    (explicitOwnerType === "group" && groupFlag === false) ||
    (explicitOwnerType === "agent" && groupFlag === true) ||
    (explicitOwnerType === "group" && agentId !== undefined) ||
    (explicitOwnerType === "agent" && groupId !== undefined) ||
    (groupFlag === true && agentId !== undefined) ||
    (groupFlag === false && groupId !== undefined)
  )
    return null;

  let ownerType: ConversationOwnerType;
  if (
    explicitOwnerType === "group" ||
    groupFlag === true ||
    groupId !== undefined
  )
    ownerType = "group";
  else if (explicitOwnerType === "agent" || agentId !== undefined)
    ownerType = "agent";
  else return null;
  if (explicitOwnerType && explicitOwnerType !== ownerType) return null;

  const resolvedOwnerId =
    ownerType === "group"
      ? firstDefined(groupId, ownerId)
      : firstDefined(agentId, ownerId);
  if (typeof resolvedOwnerId !== "string" || !resolvedOwnerId) return null;
  if (ownerId !== undefined && ownerId !== resolvedOwnerId) return null;
  return { ownerType, ownerId: resolvedOwnerId };
}

function buildParsedStreamEvent(
  event: any,
  fields: StreamEventFields,
  owner: ResolvedStreamOwner,
): ParsedStreamEvent | null {
  const identity = makeConversationIdentity(
    owner.ownerId,
    owner.ownerType,
    fields.topicId,
  );
  const messageIdentity = makeMessageIdentity(
    owner.ownerId,
    owner.ownerType,
    fields.topicId,
    fields.messageId,
  );
  if (!identity || !messageIdentity) return null;
  return {
    event,
    messageId: fields.messageId,
    identity,
    messageKey: messageIdentityKey(messageIdentity),
    topicKey: topicIdentityKey(identity),
    context: fields.context,
    generation: fields.streamGeneration ?? fields.generation,
    wireGeneration: fields.generation,
  };
}

/**
 * Resolve the owner namespace carried by legacy and Wire 1.4 stream events.
 * Incomplete or contradictory events are intentionally dropped: choosing an
 * Agent namespace here would let a Group response contaminate an Agent topic.
 */
export function parseStreamEvent(event: any): ParsedStreamEvent | null {
  const fields = readStreamEventFields(event);
  if (!fields) return null;
  const ownerFields = readStreamOwnerFields(event, fields.context);
  if (!ownerFields) return null;
  const owner = resolveStreamOwner(ownerFields);
  return owner ? buildParsedStreamEvent(event, fields, owner) : null;
}
