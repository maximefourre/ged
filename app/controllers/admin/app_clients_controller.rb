class Admin::AppClientsController < Admin::BaseController
  def index
    @app_clients = AppClient.includes(:departments).order(:client_identifier)
  end

  def new
    @app_client = AppClient.new
  end

  def edit
    @app_client = AppClient.find(params[:id])
  end

  def create
    @app_client = AppClient.new(app_client_params)
    if @app_client.save
      redirect_to admin_app_clients_path, notice: "Client registered."
    else
      flash.now[:alert] = @app_client.errors.full_messages.join(", ")
      render :new, status: :unprocessable_entity
    end
  end

  def update
    @app_client = AppClient.find(params[:id])
    if @app_client.update(app_client_params)
      redirect_to admin_app_clients_path, notice: "Client updated."
    else
      flash.now[:alert] = @app_client.errors.full_messages.join(", ")
      render :edit, status: :unprocessable_entity
    end
  end

  def destroy
    AppClient.find(params[:id]).destroy
    redirect_to admin_app_clients_path, notice: "Client deleted."
  end

  private

  def app_client_params
    params.require(:app_client)
          .permit(:client_identifier, :description, department_ids: [])
  end
end
