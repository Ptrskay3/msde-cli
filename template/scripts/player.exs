defmodule PGS.Player do
    @moduledoc """

    This is a very basic player example that shows the different type
    of data storage available.  There are three types of data storage:

    1) Player Properties:
    ---------------------

    A KV storage system where each key must be a string and the value
    must be an ATOM (which are converted to strings), NUMBER, or STRING value type.
    The values will be stored and reloaded each game session

    There are two type of properties as well:

    Local:

    These will stay on the server an not be sync'd down to the clients

    Shared:

    When added/modified/removed these values will also be sync'd down to the
    client.

    2) Player Persist data:
    -----------------------

    This is a raw blob of data you can put anything it (structs, maps, raw binary).
    This data will be stored in the database and reloaded each player session.

    3) Player Session data:
    -----------------------

    A data blob that is only good for the current game session.  Afterwards it will
    be free'd and NOT stored to the Database.

    """

    # Callback made by the system to get the profile
    # for the this player.
    # type == :public, :private, :area, :leaderboard, :presence
    def profile(:presence), do: %{name: Sys.Player.name}
    def profile(_type),     do: Sys.Player.persist

    # Serialize data for the web portal. The web portal can read the same
    # data formats, so can just pass the data through.
    def serialize(:portal, _type, data) do
        data
    end

    # Callback made by the system to covert the given
    # data term into a binary value.  The category and type
    # can be used to classify the data.
    #
    # In this demo we use Message Pack.
    def serialize(_category, _type, data) do
        Sys.Data.encode(:msgpack, data)
    end

    # Deserialize data for the web portal. The web portal can read the same
    # data formats, so can just pass the data through.
    def deserialize(:portal, _type, data) do
        data
    end

    # Callback made by the system to covert the given
    # data binary value into a system term.  The category and type
    # can be used to classify the data.
    #
    # In this demo we use Message Pack.
    def deserialize(_category, _type, data) do
        Sys.Data.decode(:msgpack, data)
    end

    # Called when a new player is created.
    def onCreate do
        Sys.Log.debug("#{Sys.Game.name}-#{Sys.Game.stage} New player (#{inspect Sys.Player.key}) has joined the game!")

        # Set the player data to equal that of the of
        # the values defined in the player.json
        defaultValues = Sys.Tuning.value("player")

        # in this case we are going to load the map from the Sys Tuning Lib
        # then iterate on each key/value in it and set our local props values to it.
        Enum.each(defaultValues, fn({key, value}) ->
            Sys.Player.setSharedProperty(key, value)
        end)

        # Lets build up the player data
        Sys.Player.setPersist(%{})
    end

    # Called when a player process is started
    def onLogin do
        Sys.Log.debug("#{Sys.Player.key} logged in to #{Sys.Game.name}-#{Sys.Game.stage}")
        :ok
    end

    def onLogout do
        Sys.Log.debug("#{Sys.Player.key} logged out of #{Sys.Game.name}-#{Sys.Game.stage}")
        :ok
    end

    def onStart do
        Sys.Log.debug("#{Sys.Player.key} Process started in to #{Sys.Game.name}-#{Sys.Game.stage}")
        :ok
    end

    def onStop(reason) do
        Sys.Log.debug("#{Sys.Player.key} Process Stopped Simple-Base because of #{inspect reason}")
        :ok
    end

    # Mine ore, called by the client with the Mine Ore button
    def onMessage("mine_ore", _data) do
        # Update the ore value
        ore = Sys.Player.sharedProperty("ore") + 1
        Sys.Player.setSharedProperty("ore", ore)
        :ok
    end

    def onMessage("sendMail", _data) do
        Sys.Player.mailSend(Sys.Player.key, "The subject!", "Hi #{Sys.Player.name}! You sent yourself a mail.")
    end

    def onMessage("sendReward", _data) do
        Sys.Player.mailSend(Sys.Player.key, "Rewards",
                                            "You got some ore!",
                                            %{"type" => "gift", "args" => %{"ore" => 5}},
                                            ["cmd"])
    end

    # Attempt to turn ore into coins.
    # Called by the client with the Mint Coins button
    def doRequest("mint_coins", _args) do
        # Read the player data
        ore = Sys.Player.sharedProperty("ore")

        # Read the tuning data
        tuning  = Sys.Tuning.value("info")

        # How many ore it takes to make coins
        cost    = tuning["convertCost"]

        # How many coins get made
        value   = tuning["convertValue"]

        # If the player has enough coins
        # Deduct the ore and add the coins
        if ore >= cost do
            # do the conversion
            Sys.Player.setSharedProperty("ore", ore - cost)
            coins = Sys.Player.sharedProperty("coins")
            Sys.Player.setSharedProperty("coins", coins + value)

            # Return a success if the player has enough
            # ore to convert to a gold coin

            # NOTE: In elixir the last line to execute will be the
            # return value for the function
            %{:results => "success"}
        else
            %{:results => "failed"}
        end
    end

    # Attempt to make bricks.
    # Called by the client with the Make Bricks button
    def doRequest("make_bricks", _args) do
        # Read the player data
        persist = Sys.Player.persist
        makingBrick = persist["makingBrick"]

        # If we aren't making a brick, start making one
        if makingBrick == nil do
            # Set when the brick was started
            persist = put_in persist, ["makingBrick"], Sys.Time.now
            # store the data
            Sys.Player.setPersist(persist)

            # NOTE: In elixir the last line to execute will be the
            # return value for the function
            %{:results => "success"}
        else
            %{:results => "failed"}
        end
    end

    # Attempt to claim a brick
    # Called by the client with the Claim Brick button
    def doRequest("claim_bricks", _args) do
        # Read the player data
        persist = Sys.Player.persist
        # Get when the brick will be completed
        makingBrick = persist["makingBrick"]

        # If we are making a brick
        if makingBrick != nil do
            # Load the tuning file
            tuning  = Sys.Tuning.value("info")
            # Get the time it takes to make a brick
            time    = tuning["brickMakeTime"]

            # If the time has passed, then claim it
            if Sys.Time.now > makingBrick + time do
                # Add one brick
                bricks  = Sys.Player.sharedProperty("bricks") + 1
                Sys.Player.setSharedProperty("bricks", bricks)

                # Remove the timer, which allows us to make another brick
                persist = Map.delete(persist, "makingBrick")

                # store the data
                Sys.Player.setPersist(persist)

                # Update the return value
                %{:results => "success"}
            else
                %{:results => "failed"}
            end
        else
            %{:results => "failed"}
        end
    end

    def doRequest("convert_gems", _args) do
        gems = Sys.Player.walletBalance()

        tuning = Sys.Tuning.value("info")
        gemsRequired = Map.get(tuning, "gemsForBricks", 1)
        if gems >= gemsRequired and :ok == Sys.Player.walletDebit(gemsRequired, "gemsToBricks") do
            bricksToAdd = tuning["bricksForGems"]
            Sys.Player.setSharedProperty("bricks", Sys.Player.sharedProperty("bricks", 0) + bricksToAdd)

            %{:results => :ok}
        else
            %{:results => :failed}
        end
    end

    def doRequest(type, _data) do
        Sys.Log.warn("Unknown Player.doRequest: #{inspect type}")
    end

    # Called when the player has requested that
    # a mail message be "executed"
    # The return value is sent to the client
    def doMailExecute(head, msg, data, args) do
        case data["type"] do
            "gift" ->
                :ok = Sys.Player.setSharedProperty("ore", Sys.Player.sharedProperty("ore") + data["args"]["ore"])
            _ -> 
                Sys.Log.debug """
                Unknown mail:
                head = #{inspect head}
                ----------------------
                msg  = #{inspect msg}
                ----------------------
                data = #{inspect data}
                ----------------------
                args = #{inspect args}
                ----------------------
                """
        end
        Sys.Player.mailDelete(head[:mailKey])
    end

    # called by the portal
    def doPortal(type, data), do: PGS.PortalHandler.handle(:player, type, data)

    def getStatusValues() do
        ["inactive", "shadowbanned"]
    end

    def allowRequest(_type, _args) do
        true
    end

    def allowMessage(_type, _args) do
        true
    end

    def allowCommand(_type, _args) do
        true
    end

    def allowNotify(_type, _args) do
        true
    end

    def onBanned() do
        Sys.Log.debug("[#{__MODULE__}.onBanned] Player has been banned!")
    end

    def onBlocked() do
        Sys.Log.debug("[#{__MODULE__}.onBlocked] Player has been blocked!")
    end

    def onStatusChange(old, new) do
        Sys.Log.debug("[#{__MODULE__}.onStatusChange] Player status has changed from #{old} to #{new}!")
    end

end # end of module player
