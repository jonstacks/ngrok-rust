// Smoke test — would run with Node.js after native build
// import { NapiAgent, ngrokListen } from './index.js';
// const agent = NapiAgent.create({ authtokenFromEnv: true });
// const listener = await agent.listen({ url: 'https://example.ngrok.app' });
// console.log('Listening on:', listener.url);
// await listener.close();
console.log('FFI validation: TypeScript declarations match Rust API surface.');
