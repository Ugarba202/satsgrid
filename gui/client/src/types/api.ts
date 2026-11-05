export interface L402Challenge {
  macaroon: string;
  invoice: string;
  parsedCaveats?: Record<string, string>;
}

export class L402Error extends Error {
  challenge: L402Challenge;

  constructor(challenge: L402Challenge) {
    super("L402 Payment Required");
    this.name = "L402Error";
    this.challenge = challenge;
  }
}
