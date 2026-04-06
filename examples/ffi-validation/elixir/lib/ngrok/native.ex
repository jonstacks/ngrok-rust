defmodule Ngrok.Native do
  @moduledoc "NIF stubs for the ngrok Rust binding."

  @doc "Create an agent with the given authtoken."
  def create_agent(_authtoken), do: :erlang.nif_error(:not_loaded)

  @doc "Create an agent reading NGROK_AUTHTOKEN from the environment."
  def create_agent_from_env(), do: :erlang.nif_error(:not_loaded)

  @doc "Start a listener on the given URL."
  def listen(_agent, _url \\ nil), do: :erlang.nif_error(:not_loaded)

  @doc "Start a forwarder to the given upstream address."
  def forward(_agent, _upstream, _url \\ nil), do: :erlang.nif_error(:not_loaded)
end
