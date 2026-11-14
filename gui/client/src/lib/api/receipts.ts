import { fetchWithL402 } from './client';

export interface ReceiptPayload {
  receipt_id: string;
  schema_version: number;
  key_id: string;
  provider_id: string;
  request_id: string;
  session_id?: string;
  model: string;
  inference_status: string;
  payment_status: string;
  payment_hash?: string;
  approved_max_sats: number;
  actual_charge_sats: number;
  currency: string;
  prompt_tokens?: number;
  completion_tokens?: number;
  error_code?: string;
  reconciliation_id?: string;
  supersedes_receipt_id?: string;
  created_at: number;
}

export interface SignedReceipt {
  payload: ReceiptPayload;
  signature_hex: string;
  public_key_hex: string;
  algorithm: string;
  key_id: string;
}

export async function getReceipt(receiptId: string): Promise<SignedReceipt> {
  return fetchWithL402(`/v1/receipts/${encodeURIComponent(receiptId)}`);
}
