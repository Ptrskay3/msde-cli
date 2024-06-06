defmodule PGS.PortalHandler do

    #--------------------------------------------------------------------------
    @doc """

    """
    def handle(_, "print_debug",data),  do: Sys.Log.debug(data["text"])
    def handle(_, "print_info", data),  do: Sys.Log.info(data["text"])
    def handle(_, "print_warn", data),  do: Sys.Log.warn(data["text"])
    def handle(_, "print_error",data),  do: Sys.Log.error(data["text"])

    #--------------------------------------------------------------------------
    @doc """

    """
    def handle(:game, "announcement", data) do
        Sys.Game.publishAnnouncement(data["message"])
        Sys.Game.setOfflineMessage(data["message"])
        :ok
    end

    def handle(:player, "reward_pack1", data) do
        Sys.Log.debug("reward_pack1 data: #{inspect data}")
        rewardPacks = Sys.Tuning.value("DynamicOption3")
        pack = data["pack"]
        delta =
        if String.contains?(pack, "daily_") do
            # daily pack
            rewardPacks["daily"][pack]["amount"]
        else
            # weekly pack
            rewardPacks["weekly"][pack]["amount"]
        end
        ore = Sys.Player.sharedProperty("ore") + delta
        Sys.Player.setSharedProperty("ore", ore)
        ore
    end

    def handle(:player, "reward_pack2and3", data) do
        Sys.Log.debug("reward_pack2 data: #{inspect data}")
        rewardPacks = Sys.Tuning.value("DynamicOption3")
        delta =
        if nil != data["dailyPack"] do
            pack = data["dailyPack"]
            # daily pack
            rewardPacks["daily"][pack]["amount"]
        else
            pack = data["weeklyPack"]
            # weekly pack
            rewardPacks["weekly"][pack]["amount"]
        end
        ore = Sys.Player.sharedProperty("ore") + delta
        Sys.Player.setSharedProperty("ore", ore)
        ore
    end

    def handle(:player, "reward_pack4", data) do
        Sys.Log.debug("reward_pack4 data: #{inspect data}")
        index = String.to_integer(data["pack"])
        rewardPacks = Sys.Tuning.value("DynamicOption4")
        delta = Enum.at(rewardPacks, index)["amount"]
        ore = Sys.Player.sharedProperty("ore") + delta
        Sys.Player.setSharedProperty("ore", ore)
        ore
    end

    def handle(:player, "reward_pack5", data) do
        Sys.Log.debug("reward_pack5 data: #{inspect data}")
        rewardPacks = Sys.Tuning.value("DynamicOption5")
        delta =
        if nil != data["dailyPack"] do
            index = String.to_integer(data["dailyPack"])
            # daily pack
            Enum.at(rewardPacks["daily"], index)["amount"]
        else
            index = String.to_integer(data["weeklyPack"])
            # weekly pack
            Enum.at(rewardPacks["weekly"], index)["amount"]
        end
        ore = Sys.Player.sharedProperty("ore") + delta
        Sys.Player.setSharedProperty("ore", ore)
        ore
    end

    #--------------------------------------------------------------------------
    @doc """

    """
    def handle(:player, "set_name", data) do
        Sys.Player.setName(data["name"])
    end

    #--------------------------------------------------------------------------
    @doc """

    """
    def handle(:player, "session", _data) do
        Sys.Player.Session.length
    end

    #--------------------------------------------------------------------------
    @doc """

    """
    def handle(:player, "modify_ore", data) do
        delta = String.to_integer(data["arg0"])
        ore = Sys.Player.sharedProperty("ore") + delta
        Sys.Player.setSharedProperty("ore", ore)
        %{"ore" => ore}
    end

    #--------------------------------------------------------------------------
    @doc """

    """
    def handle(:player, "send_mail", data) do
        Sys.Player.mailSend(Sys.Player.key, data["subject"], data["message"])
    end

    #--------------------------------------------------------------------------
    @doc """

    """
    def handle(:player, "send_mail_reward", data) do
        ore = String.to_integer(data["ore"])
        mailData = %{"type" => "gift", "args" => %{"ore" => ore}}
        Sys.Player.mailSend(Sys.Player.key, data["subject"], data["message"], mailData, ["cmd"])
    end

    #--------------------------------------------------------------------------
    @doc """

    """
    def handle(:game, "mail_send", data) do
        res = Sys.Mailbox.send(String.to_integer(data["pKey"]), -1, "Test Message", "Ready Player One!", Sys.Data.encode(:msgpack, %{"test" => "value"}), ["cmd"], -1)
        Sys.Log.info("Output = #{inspect res}")
        :ok
    end

    #--------------------------------------------------------------------------
    @doc """

    """
    def handle(:player, "mail_process", _data) do
        case Sys.Player.mailItems() do
            :failed -> Sys.Log.error("[PGS.Portal.Handler] mailprocess: Failed to pull mailbox items")
            []      -> Sys.Log.debug("[PGS.Portal.Handler] mailprocess: No items to process")
            mails   ->  for mail <- mails do
                            Sys.Player.mailExecute(Map.get(mail, :mailKey, Map.get(mail, "mailKey")), %{:cmd => "myArgs"})
                        end
        end
        :ok
    end

    #--------------------------------------------------------------------------
    @doc """

    """
    def handle(:player, "ban_player", data) do
        Sys.Log.debug("[#{__MODULE__}.ban_player] data: #{inspect data}")
        Sys.Player.setStatusToBanned("Banned by portal")
    end

    #--------------------------------------------------------------------------
    @doc """

    """
    def handle(:player, "block_player", data) do
        Sys.Log.debug("[#{__MODULE__}.block_player] data: #{inspect data}")
        Sys.Player.setStatusToBlocked("Blocked by portal")
    end

    #--------------------------------------------------------------------------
    @doc """

    """
    def handle(:player, "active_player", _data) do
        oldStatus = Sys.Player.status()
        Sys.Log.debug("[#{__MODULE__}.active_player] #{oldStatus}")
        Sys.Player.setStatusToActive("Active by portal")
    end

    def handle(:game, "triggerAction", data) do
        # fire off the action in 1 second.  Note if you need such a fine grain
        # timer you should really be using "Sys.Time.callAfter" but for the
        # purposes of this example it should be fine.
        Sys.Game.scheduleAction(1000, "testAction", data)
        :ok
    end

    def handle(:game, "triggerEvent", data) do
        # fire off the action in 1 second.  Note if you need such a fine grain
        # timer you should really be using "Sys.Time.callAfter" but for the
        # purposes of this example it should be fine.
        _taskId = Sys.Game.scheduleEvent(1000, 2000, 5000, 2, "testEvent", data)
        :ok
    end

    #--------------------------------------------------------------------------
    @doc """

    """
    def handle(type, request, _data) do
        Sys.Log.error("[PGS.Portal.Handler] type = #{type} request = #{request}")
        %{"status" => :failed, "reason" => "unknown portal request"}
    end
end
