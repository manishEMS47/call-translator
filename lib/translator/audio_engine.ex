defmodule Translator.AudioEngine do
  @moduledoc """
  GenServer wrapping the Rust audio_engine binary via an Erlang Port.

  Communicates using 4-byte length-prefixed JSON protocol (`{:packet, 4}`).
  Handles Port crashes with automatic restart after a delay.
  """

  use GenServer
  require Logger

  @restart_delay_ms 2_000

  @log_file "test-log.txt"

  defstruct [:port, status: :idle, pipelines: [], devices: %{"input" => [], "output" => []}]

  # --- Client API ---

  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @spec start_pipelines(list(String.t())) :: :ok | {:error, term()}
  def start_pipelines(pipelines \\ ["outgoing", "incoming"]) do
    config = Translator.Config.get_all()

    GenServer.call(__MODULE__, {:start_pipelines, pipelines, config})
  end

  @spec stop_pipelines() :: :ok | {:error, term()}
  def stop_pipelines do
    GenServer.call(__MODULE__, :stop_pipelines)
  end

  @spec status() :: map()
  def status do
    GenServer.call(__MODULE__, :status)
  end

  @spec set_config(atom(), term()) :: :ok | {:error, term()}
  def set_config(key, value) do
    Translator.Config.put(key, value)

    GenServer.call(__MODULE__, {:set_config, key, value})
  end

  @spec send_command(map()) :: :ok | {:error, term()}
  def send_command(command) when is_map(command) do
    GenServer.call(__MODULE__, {:send_command, command})
  end

  @spec pop_audio() :: list(map())
  def pop_audio do
    GenServer.call(__MODULE__, :pop_audio)
  end

  @spec get_devices() :: map()
  def get_devices do
    GenServer.call(__MODULE__, :get_devices)
  end

  @spec restart_engine_async() :: :ok
  def restart_engine_async do
    GenServer.cast(__MODULE__, :restart_engine)
  end

  # --- Server Callbacks ---

  @impl true
  def init(_opts) do
    case open_port() do
      {:ok, port} ->
        Logger.info("AudioEngine started, port opened")
        {:ok, %__MODULE__{port: port, status: :idle}}

      {:error, reason} ->
        Logger.error("AudioEngine failed to open port: #{inspect(reason)}")
        {:ok, %__MODULE__{port: nil, status: :crashed}}
    end
  end

  @impl true
  def handle_call({:start_pipelines, _pipelines, _config}, _from, %{port: nil} = state) do
    Logger.error("Cannot start pipelines: engine port is not open")
    {:reply, {:error, :port_not_open}, state}
  end

  def handle_call({:start_pipelines, pipelines, config}, _from, state) do
    command = %{
      "cmd" => "start",
      "pipelines" => pipelines,
      "config" => encode_config(config)
    }

    send_to_port(state.port, command)
    {:reply, :ok, %{state | status: :starting, pipelines: pipelines}}
  end

  @impl true
  def handle_call(:stop_pipelines, _from, %{port: nil} = state) do
    {:reply, {:error, :port_not_open}, state}
  end

  def handle_call(:stop_pipelines, _from, state) do
    command = %{"cmd" => "stop"}
    send_to_port(state.port, command)
    {:reply, :ok, %{state | status: :stopping}}
  end

  @impl true
  def handle_call(:status, _from, state) do
    {:reply, %{status: state.status, pipelines: state.pipelines}, state}
  end

  @impl true
  def handle_call({:set_config, _key, _value}, _from, %{port: nil} = state) do
    {:reply, {:error, :port_not_open}, state}
  end

  def handle_call({:set_config, key, value}, _from, state) do
    command = %{
      "cmd" => "set_config",
      "key" => to_string(key),
      "value" => value
    }

    send_to_port(state.port, command)
    {:reply, :ok, state}
  end

  @impl true
  def handle_call(:pop_audio, _from, state) do
    queue = Process.get(:audio_queue, [])
    Process.put(:audio_queue, [])
    {:reply, queue, state}
  end

  @impl true
  def handle_call(:get_devices, _from, state) do
    {:reply, state.devices, state}
  end

  @impl true
  def handle_call({:send_command, _command}, _from, %{port: nil} = state) do
    {:reply, {:error, :port_not_open}, state}
  end

  def handle_call({:send_command, command}, _from, state) do
    send_to_port(state.port, command)
    {:reply, :ok, state}
  end

  # --- Port message handling ---

  @impl true
  def handle_cast(:restart_engine, state) do
    Logger.info("Restarting engine (async)...")

    if state.port do
      try do
        send_to_port(state.port, %{"cmd" => "stop"})
        Process.sleep(500)
        Port.close(state.port)
      rescue
        _ -> :ok
      catch
        _, _ -> :ok
      end
    end

    Process.sleep(1000)

    case open_port() do
      {:ok, port} ->
        Logger.info("Engine restarted, starting pipelines...")
        config = Translator.Config.get_all()

        command = %{
          "cmd" => "start",
          "pipelines" => ["outgoing", "incoming"],
          "config" => encode_config(config)
        }

        send_to_port(port, command)
        {:noreply, %{state | port: port, status: :starting, pipelines: ["outgoing", "incoming"]}}

      {:error, reason} ->
        Logger.error("Failed to restart engine: #{inspect(reason)}")
        Process.send_after(self(), :restart_engine, @restart_delay_ms)
        {:noreply, %{state | port: nil, status: :crashed, pipelines: []}}
    end
  end

  @impl true
  def handle_info({port, {:data, data}}, %{port: port} = state) do
    case Jason.decode(data) do
      {:ok, event} ->
        new_state = dispatch_event(event, state)
        {:noreply, new_state}

      {:error, reason} ->
        Logger.warning("Failed to decode JSON from engine: #{inspect(reason)}")
        {:noreply, state}
    end
  end

  @impl true
  def handle_info({port, {:exit_status, status}}, %{port: port} = state) do
    Logger.error("AudioEngine Rust process exited with status #{status}")
    Process.send_after(self(), :restart_engine, @restart_delay_ms)
    {:noreply, %{state | port: nil, status: :crashed}}
  end

  @impl true
  def handle_info(:restart_engine, state) do
    Logger.info("Attempting to restart AudioEngine Rust process...")

    case open_port() do
      {:ok, port} ->
        Logger.info("AudioEngine restarted successfully")
        {:noreply, %{state | port: port, status: :idle}}

      {:error, reason} ->
        Logger.error("Failed to restart AudioEngine: #{inspect(reason)}")
        Process.send_after(self(), :restart_engine, @restart_delay_ms)
        {:noreply, %{state | port: nil, status: :crashed}}
    end
  end

  @impl true
  def handle_info(msg, state) do
    Logger.debug("AudioEngine received unexpected message: #{inspect(msg)}")
    {:noreply, state}
  end

  # --- Private Helpers ---

  defp open_port do
    engine_path = Application.get_env(:translator, :audio_engine_path)
    settings = read_settings()
    models_base = System.get_env("TRANSLATOR_MODELS_DIR", "./models")

    my_lang = Map.get(settings, "my_language", "ru")
    their_lang = Map.get(settings, "their_language", "en")

    # Outgoing TTS = their language, Incoming TTS = my language
    out_voice = Map.get(settings, "tts_outgoing_voice", "")
    in_voice = Map.get(settings, "tts_incoming_voice", "")

    # Default voices if not set
    out_voice = if out_voice == "", do: default_voice(models_base, their_lang), else: out_voice
    in_voice = if in_voice == "", do: default_voice(models_base, my_lang), else: in_voice

    if engine_path && File.exists?(engine_path) do
      port =
        Port.open({:spawn_executable, engine_path}, [
          :binary,
          {:packet, 4},
          :exit_status,
          {:env, [
            {~c"RUST_LOG", ~c"warn"},
            {~c"DEEPGRAM_API_KEY", charlist_setting(settings, "deepgram_api_key", "DEEPGRAM_API_KEY")},
            {~c"SIXTYDB_API_KEY", charlist_setting(settings, "sixtydb_api_key", "SIXTYDB_API_KEY")},
            {~c"GROQ_API_KEY", charlist_setting(settings, "groq_api_key", "GROQ_API_KEY")},
            {~c"TRANSLATOR_STT_PROVIDER_OUT", String.to_charlist(Map.get(settings, "stt_provider_outgoing", "deepgram"))},
            {~c"TRANSLATOR_STT_PROVIDER_IN", String.to_charlist(Map.get(settings, "stt_provider_incoming", "deepgram"))},
            {~c"TRANSLATOR_TTS_PROVIDER_OUT", String.to_charlist(Map.get(settings, "tts_provider_outgoing", "piper"))},
            {~c"TRANSLATOR_TTS_PROVIDER_IN", String.to_charlist(Map.get(settings, "tts_provider_incoming", "piper"))},
            {~c"TRANSLATOR_TTS_60DB_VOICE_OUT", String.to_charlist(Map.get(settings, "tts_60db_voice_outgoing", ""))},
            {~c"TRANSLATOR_TTS_60DB_VOICE_IN", String.to_charlist(Map.get(settings, "tts_60db_voice_incoming", ""))},
            {~c"TRANSLATOR_TTS_EN_MODEL", String.to_charlist("#{models_base}/piper-#{their_lang}/#{out_voice}.onnx")},
            {~c"TRANSLATOR_TTS_EN_CONFIG", String.to_charlist("#{models_base}/piper-#{their_lang}/#{out_voice}.onnx.json")},
            {~c"TRANSLATOR_TTS_RU_MODEL", String.to_charlist("#{models_base}/piper-#{my_lang}/#{in_voice}.onnx")},
            {~c"TRANSLATOR_TTS_RU_CONFIG", String.to_charlist("#{models_base}/piper-#{my_lang}/#{in_voice}.onnx.json")},
            {~c"TRANSLATOR_MIC_DEVICE", String.to_charlist(Map.get(settings, "mic_device", "default"))},
            {~c"TRANSLATOR_SPEAKER_DEVICE", String.to_charlist(Map.get(settings, "speaker_device", "default"))},
            {~c"TRANSLATOR_MEET_INPUT", String.to_charlist(Map.get(settings, "meet_input_device", "BlackHole 16ch"))},
            {~c"TRANSLATOR_MEET_OUTPUT", String.to_charlist(Map.get(settings, "meet_output_device", "BlackHole 2ch"))},
            {~c"TRANSLATOR_ENDPOINTING_MS", String.to_charlist("#{Map.get(settings, "endpointing_ms", 300)}")},
            {~c"TRANSLATOR_MY_LANG", String.to_charlist(Map.get(settings, "my_language", "ru"))},
            {~c"TRANSLATOR_THEIR_LANG", String.to_charlist(Map.get(settings, "their_language", "en"))}
          ]}
        ])

      send_to_port(port, %{"cmd" => "ping"})
      {:ok, port}
    else
      Logger.warning(
        "AudioEngine binary not found at #{inspect(engine_path)}. " <>
          "Run `mix compile` to build the Rust binary."
      )

      {:error, :binary_not_found}
    end
  end

  defp read_settings do
    settings_path = Path.join(File.cwd!(), "settings.json")

    case File.read(settings_path) do
      {:ok, contents} ->
        case Jason.decode(contents) do
          {:ok, settings} -> settings
          _ -> %{}
        end

      _ ->
        %{}
    end
  end

  defp default_voice(models_base, lang) do
    dir = Path.join(models_base, "piper-#{lang}")

    case File.ls(dir) do
      {:ok, files} ->
        files
        |> Enum.filter(&String.ends_with?(&1, ".onnx"))
        |> Enum.reject(&String.ends_with?(&1, ".onnx.json"))
        |> Enum.sort()
        |> List.first("")
        |> String.replace(".onnx", "")

      _ ->
        ""
    end
  end

  defp charlist_setting(settings, json_key, env_var) do
    val = Map.get(settings, json_key, "")
    val = if val == "", do: System.get_env(env_var, ""), else: val
    String.to_charlist(val)
  end

  defp send_to_port(port, command) when is_port(port) do
    json = Jason.encode!(command)
    Port.command(port, json)
  rescue
    e ->
      Logger.error("Failed to send command to port: #{inspect(e)}")
      :error
  end

  defp send_to_port(nil, _command) do
    Logger.error("Cannot send command: port is not open")
    :error
  end

  defp dispatch_event(%{"event" => "pong"}, state) do
    Logger.debug("Received pong from engine")
    state
  end

  defp dispatch_event(%{"event" => "started", "pipelines" => pipelines}, state) do
    Logger.info("Engine started pipelines: #{inspect(pipelines)}")
    %{state | status: :running, pipelines: pipelines}
  end

  defp dispatch_event(%{"event" => "stopped"}, state) do
    Logger.info("Engine stopped all pipelines")
    %{state | status: :idle, pipelines: []}
  end

  defp dispatch_event(
         %{"event" => "transcript", "direction" => direction, "text" => text} = event,
         state
       ) do
    line = "🎤 [#{direction}] #{text}"
    Logger.info(line)
    log_to_file(line)
    notify_pipeline(direction, event)
    state
  end

  defp dispatch_event(
         %{"event" => "translation", "direction" => direction, "text" => text} = event,
         state
       ) do
    line = "🌐 [#{direction}] #{text}"
    Logger.info(line)
    log_to_file(line)
    notify_pipeline(direction, event)
    state
  end

  defp dispatch_event(%{"event" => "metrics"} = event, state) do
    metrics = Map.delete(event, "event")
    stt = Map.get(metrics, "stt_ms", 0)
    trl = Map.get(metrics, "translate_ms", 0)
    tts = Map.get(metrics, "tts_ms", 0)
    line = "⏱  stt=#{stt}ms trl=#{trl}ms tts=#{tts}ms"
    Logger.info(line)
    log_to_file(line <> "\n")
    state
  end

  defp dispatch_event(%{"event" => "error", "message" => message}, state) do
    Logger.error("Engine error: #{message}")
    state
  end

  defp dispatch_event(%{"event" => "log", "level" => level, "message" => message}, state) do
    case level do
      "debug" -> Logger.debug("Engine: #{message}")
      "info" -> Logger.info("Engine: #{message}")
      "warn" -> Logger.warning("Engine: #{message}")
      "error" -> Logger.error("Engine: #{message}")
      _ -> Logger.info("Engine [#{level}]: #{message}")
    end

    state
  end

  defp dispatch_event(%{"event" => "device_list", "input" => input, "output" => output}, state) do
    Logger.info("Received device list: #{length(input)} input, #{length(output)} output")
    %{state | devices: %{"input" => input, "output" => output}}
  end

  defp dispatch_event(
         %{"event" => "tts_audio", "direction" => _dir, "sample_rate" => sr, "audio_b64" => b64},
         state
       ) do
    # Store in process dict for polling by web UI (not in log — too large for SSE)
    queue = Process.get(:audio_queue, [])
    Process.put(:audio_queue, queue ++ [%{"sr" => sr, "b64" => b64}])
    # Keep max 5
    if length(queue) > 5 do
      Process.put(:audio_queue, Enum.take(queue, -5))
    end
    state
  end

  defp dispatch_event(%{"event" => "tts_preview_done"}, state) do
    Logger.info("TTS preview playback finished")
    state
  end

  defp dispatch_event(event, state) do
    Logger.debug("Unhandled engine event: #{inspect(event)}")
    state
  end

  defp notify_pipeline(direction, event) do
    case find_pipeline_pid(direction) do
      nil -> :ok
      pid -> Translator.Pipeline.handle_event(pid, event)
    end
  end

  defp find_pipeline_pid(direction) do
    direction_atom = String.to_existing_atom(direction)

    Translator.PipelineSupervisor.which_pipelines()
    |> Enum.find_value(fn {pid, dir} ->
      if dir == direction_atom, do: pid
    end)
  rescue
    ArgumentError -> nil
  end

  defp log_to_file(line) do
    timestamp = DateTime.utc_now() |> DateTime.to_iso8601()
    File.write(@log_file, "[#{timestamp}] #{line}\n", [:append])
  end

  defp encode_config(config) when is_map(config) do
    config
    |> Enum.map(fn {k, v} -> {to_string(k), v} end)
    |> Map.new()
  end
end
