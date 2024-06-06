defmodule PGS.Bot do
    
    #==========================================================================
    # Module consts
    #==========================================================================
    @fixedDelay 500
    @randomDelay 500

    #==========================================================================
    # Public API
    #==========================================================================
    
    #==========================================================================
    @doc """
    Callback function for a bot that defines the test "types" available to be run
    The data here will be shown in the portal and allow you to select the bot test
    you would like to run.
    """
    @spec types()::[String.t]
    def types() do
        [
            "RandomAction"
        ]
    end

    #==========================================================================
    @doc """
    callback function made when some data needs to be converted into a serialized
    form.  For this example we are going to assume that all data will be converted
    into a msgpack format
    """
    @spec serialize(atom, atom, term)::binary 
    def serialize(_category, _type, data) do
        Sys.Data.encode(:msgpack, data)
    end

    #==========================================================================
    @doc """
    callback function for when some data recieved from the game server needs
    to be converted into a format the bot client can understand.  In the simple
    examples all the data is sent in a msgpack format so we just need to
    decode it and move on
    """
    @spec serialize(atom, atom, binary)::term
    def deserialize(_category, _type, data) do
        Sys.Data.decode(:msgpack, data)
    end

    #==========================================================================
    @doc """
    Callback made when the bot process has started.  For the most part all you
    want to do from here is log the client in.  However if you want to load
    some data or generated something on your own you may do so here.
    """
    @spec onStart() :: :ok
    def onStart() do
        # Lets log this bot into the system
        Sys.Bot.Player.Commands.login()
        :ok
    end
    
    #==========================================================================
    @doc """
    Callback made when the client is fully logged in to the system
    """
    @spec onLogin() :: :ok
    def onLogin(), do: runLoop()

    
    #==========================================================================
    @doc """
    callback made when the server response to a server request (or if it fails
    to send it at all).
    """
    @spec onRequest(String.t, term, atom) :: :ok
    def onRequest("mint_coins", dataOut, err) do
        cond do
            err == :none and dataOut["results"] == "ok" ->
                wait()
                runLoop() 
            err != :none ->
                Sys.Log.error("RequestDelay bot request failed: #{inspect err}")
                Sys.Bot.finish()
            true ->
                Sys.Log.error("RequestDelay bot request failed: #{inspect dataOut}")
                Sys.Bot.finish()
        end
        :ok
    end

    #==========================================================================
    # Module Private APIS
    #==========================================================================
    
    #==========================================================================
    defp runLoop() do
        # Callback maded when the bot successfully logs into the system
        Sys.Bot.Player.Commands.message("mine_ore", %{})
        Sys.Bot.Player.Commands.message("mine_ore", %{})
        Sys.Bot.Player.Commands.message("mine_ore", %{})
        Sys.Bot.Player.Commands.message("mine_ore", %{})
        Sys.Bot.Player.Commands.message("mine_ore", %{})
        # Mine fine ore and convert it to a 
        Sys.Bot.Player.Commands.request("mint_coins", %{}, Sys.cb(:onRequest)) 
    end

    #==========================================================================
    defp wait, do: Sys.Time.wait(@fixedDelay + Sys.Random.int(@randomDelay))
end
