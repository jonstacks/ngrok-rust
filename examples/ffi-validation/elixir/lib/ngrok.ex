defmodule Ngrok do
  @moduledoc "High-level Elixir API wrapping ngrok NIFs."

  @doc "Connect an agent and start listening."
  def listen(opts \\ []) do
    url = Keyword.get(opts, :url)
    with {:ok, agent} <- Ngrok.Native.create_agent_from_env(),
         {:ok, info} <- Ngrok.Native.listen(agent, url) do
      {:ok, info}
    end
  end

  @doc "Connect an agent and start forwarding."
  def forward(upstream, opts \\ []) do
    url = Keyword.get(opts, :url)
    with {:ok, agent} <- Ngrok.Native.create_agent_from_env(),
         {:ok, info} <- Ngrok.Native.forward(agent, upstream, url) do
      {:ok, info}
    end
  end
end
