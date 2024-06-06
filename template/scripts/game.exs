defmodule PGS.Game do

    # Callback made each time a game has been started.
    def onStart() do
        Sys.Log.debug("#{Sys.Game.name}-#{Sys.Game.stage} Started!")
    end

    # Callback made when a tuning package has been updated
    def onTuningUpdate(_keys) do
    	Sys.Log.debug("[PGS.Game] Tuning Updated for #{Sys.Game.name}-#{Sys.Game.stage}")

        # Lets only push tuning if the valid is successful
        if PGS.Schema.validateTuning(), do: pushTuningChanges()
    end

    # called by the portal
    def doPortal(type, data), do: PGS.PortalHandler.handle(:game, type, data)

    defp pushTuningChanges() do
    	Sys.Log.debug("[PGS.Game] Publishing a new CMS package for #{Sys.Game.name}-#{Sys.Game.stage}")
        # Publish the tuning to the CDN
        # Creates a package called "tuning"
        # Adds a key called "info"
        # Puts the data from the tuning directory in info.json into that key
        #
        # The client needs to look for pacakge tuning, inside of which is a
        # key called "info", which has a copy of the data stored in info.json
        # Additionally, there is a key "bank" which has the bank data
        tuningData = %{ "info"=>  Sys.Data.encode(:msgpack, Sys.Tuning.value("info")),
                        "bank" => Sys.Data.encode(:msgpack, Sys.Tuning.value("pg_bank"))}

        # And messages is a set of data that is generated from code.
        # One really cool thing you do with this system is create some procedurally
        # generated content (like a dungeon, etc) and push that data to the CDN
        # that way you don't have to burn cpu cycles or bandwidth on your servers
        tuningMsg  = %{
                        "greeting" => "Hello World",
                        "farewell" => "See you next time"
                     }

        # Because we want multiple packages to go "live" at the same time there is a concept
        # of a "deployment".  In this example we are creating the deployment, uploading
        # the packages and publishing it all in one chain of operations but
        # they can be broken out and done individual over an X timespan

        :ok = Sys.Assets.Deployment.new                         # Creates a new deployment
            |> Sys.Assets.Deployment.add("tuning", tuningData)  # adds/updates the package tuning
            |> Sys.Assets.Deployment.add("messages",tuningMsg)  # adds/updates the package tuning
            |> Sys.Assets.Deployment.close                      # Locks the deployment down
            |> Sys.Assets.Deployment.publish                    # Pushs the data out live


        # TODO
        # Alternative version of code. We can change for readability if we don't
        # want to get into using the Pipe operator for elixir

        #deployId = Sys.Assets.Deployment.new
        #Sys.Assets.Deployment.addPackage(deployId, "tuning", tuningData)
        #Sys.Assets.Deployment.addPackage(deployId, "messages",tuningMsg)
        #:ok = Sys.Assets.Deployment.publish(deployId)
        :ok
    end

    def onAction("testAction", _args, _actionId) do
        Sys.Game.publishAnnouncement("Test Action!")
    end

    def onAction(type, args, _actionId) do
        Sys.Log.warn("Unhandled action: #{inspect type}, #{inspect args}")
    end

    def onEvent("testEvent", event, args, id) do
        now = Sys.Time.now
        Sys.Log.debug(
        """
        testEvent #{event} triggered at #{now}
        Args = #{inspect args}
        ID   = #{inspect id}
        """)
        announcement = case event do
            :start  -> "Test event started!"
            :end    -> "Test event ended!"
        end
        Sys.Game.publishAnnouncement(announcement)
        :ok
    end

    def onEvent(type, _startStop, args, _actionId) do
        Sys.Log.warn("Unhandled event: #{inspect type}, #{inspect args}")
    end
end # end of module game

