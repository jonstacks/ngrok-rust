/** Agent configuration and connection management. */
export class NapiAgent {
  /** Create an agent with the given options. */
  static create(opts: { authtoken?: string; authtokenFromEnv?: boolean }): NapiAgent;
  /** Listen for incoming connections. */
  listen(opts?: { url?: string; trafficPolicy?: string; metadata?: string }): Promise<NapiListener>;
  /** Forward connections to an upstream address. */
  forward(opts: { addr: string; url?: string; trafficPolicy?: string }): Promise<NapiForwarder>;
}

/** An ngrok endpoint listener. */
export class NapiListener {
  /** The endpoint's unique ID. */
  readonly id: string;
  /** The endpoint's public URL. */
  readonly url: string;
  /** The endpoint's protocol. */
  readonly protocol: string;
  /** Close the listener. */
  close(): Promise<void>;
}

/** An ngrok endpoint forwarder. */
export class NapiForwarder {
  /** The endpoint's unique ID. */
  readonly id: string;
  /** The endpoint's public URL. */
  readonly url: string;
  /** The upstream URL. */
  readonly upstreamUrl: string;
  /** Close the forwarder. */
  close(): Promise<void>;
}

/** Listen using the default agent (reads NGROK_AUTHTOKEN from env). */
export function ngrokListen(opts?: { url?: string }): Promise<NapiListener>;
/** Forward using the default agent. */
export function ngrokForward(opts: { addr: string; url?: string }): Promise<NapiForwarder>;
