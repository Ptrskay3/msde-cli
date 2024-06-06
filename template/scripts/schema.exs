defmodule PGS.Schema do
    
    @prod_stage 1234


    # Schema Setup for the Bank
    schema(:info)do
        [
            field(name: "convertCost",      type: :number,     values: [:integer, :pos, {:less, 127}]),
            field(name: "convertValue",     type: :number,     values: [:integer, :pos, {:less, 127}]),
            field(name: "brickMakeTime",    type: :number,     values: [:integer, :pos, {:less, 60_000}]),
            field(name: "bricksForGems",    type: :number,     values: [:integer, :pos, {:less, 50}], optional: true),
            field(name: "gemsForBricks",    type: :number,     values: [:integer, :pos, {:less, 50}], optional: true),
            field(name: "mineCost",         type: :number,     values: [:integer, :pos, {:less, 127}]),
            field(name: "mineTime",         type: :number,     values: [:integer, :pos, {:less, 120_000}]),
            field(name: "campCost",         type: :number,     values: [:integer, :pos, {:less, 10}]),
            field(name: "areaWidth",        type: :number,     values: [:integer, :pos, {:less, 16}]),
            field(name: "areaHeight",       type: :number,     values: [:integer, :pos, {:less, 16}])
        ]     
    end 
    
    schema(:player)do
        [
            field(name: "ore",      type: :number,     values: [:integer, :pos, {:less, 100}]),
            field(name: "coins",    type: :number,     values: [:integer, :pos, {:less, 100}]),
            field(name: "bricks",   type: :number,     values: [:integer, :pos, {:less, 100}]),
            field(name: "base",     type: :number,     values: [:integer, :pos, {:less, 10}]),
            field(name: "camp",     type: :number,     values: [:integer, :pos, {:less, 10}]),
            field(name: "claim",    type: :number,     values: [:integer, :pos, {:less, 1}]),
        ]     
    end 
    
    schema(:sample)do
        [
            field(name: "name",     type: :string,      values: [{:max_len, 128}, {:apply, :nameFieldCheck}]),
            field(name: "address",  type: :string,      values: [{:hard, ["california", "florida"]}]),
        ]     
    end 

    def nameFieldCheck(value) do
        # lets make suer all values are lowercase.
        # Not really a great check you would want to do
        value == String.downcase(value)
    end

    def validate(), do: validateTuning()

    ##===================================================================
    @doc """
    """
    @spec validateTuning() :: true | false
    def validateTuning() do
        if( Sys.Game.key == @prod_stage ) do
            true
        else
            cond do
                true != validateWithSchema(Sys.Tuning.value("info"), :info) ->
                    Sys.Log.error "[PGS.Schema] Failed to validate tuning file \"info.json\"" 
                    false 
                
                true != validateWithSchema(Sys.Tuning.value("player"), :player) ->
                    Sys.Log.error "[PGS.Schema] Failed to validate tuning file \"player.json\"" 
                    false 
                
                true != validateWithSchema(Sys.Tuning.value("sample"), :sample) ->
                    Sys.Log.error "[PGS.Schema] Failed to validate tuning file \"sample.csv\"" 
                    false 
                
                true ->
                    Sys.Log.info "[PGS.Schema] Tuning schema validation successful" 
                    true
            end
        end
    end
end
